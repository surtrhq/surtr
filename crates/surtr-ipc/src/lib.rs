//! Local IPC between `surtr serve` and its front ends (plan, sections 4 and
//! 5.6).
//!
//! **Protocol:** one JSON object per line. The client first sends `hello`
//! and checks [`PROTOCOL`]; then it calls methods by name. Requests carry an
//! id that the response repeats.
//!
//! **Security:** the endpoint is reachable only by the user who started the
//! daemon. On Unix it is a socket (mode 0600) in a private directory (mode
//! 0700), and every connection's peer user id is checked. On Windows it is a
//! named pipe whose access list grants only the current user (the default
//! would also let Everyone read), that rejects remote clients and that must
//! be the first instance of its name.
//!
//! `SURTR_IPC_ENDPOINT` replaces the endpoint name (tests, side-by-side
//! daemons).

use std::{env, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod client;
mod server;
#[cfg(windows)]
mod windows;

pub use client::Client;
pub use server::{Handler, serve};

/// Version of the protocol; a mismatch ends the conversation.
pub const PROTOCOL: u32 = 1;

/// Longest request line the server accepts, in bytes.
pub const MAX_REQUEST: usize = 64 * 1024;

/// A call from a front end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Echoed in the response.
    pub id: u64,
    /// What to do, for example `hello` or `read`.
    pub method: String,
}

/// The daemon's answer: exactly one of `ok` and `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// The request's id.
    pub id: u64,
    /// The result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `hello` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// [`PROTOCOL`] of the daemon.
    pub protocol: u32,
    /// Surtr version of the daemon.
    pub version: String,
    /// Provider ids the daemon reads from.
    pub providers: Vec<String>,
}

/// Where the daemon listens. On Unix a socket path, on Windows a pipe name.
pub fn endpoint() -> io::Result<PathBuf> {
    let custom = env::var("SURTR_IPC_ENDPOINT")
        .ok()
        .filter(|v| !v.is_empty());
    #[cfg(windows)]
    {
        let name = match custom {
            Some(name) => name,
            None => format!("surtr-serve-{}", windows::user_sid()?),
        };
        Ok(PathBuf::from(format!(r"\\.\pipe\{name}")))
    }
    #[cfg(unix)]
    {
        // XDG_RUNTIME_DIR is per-user and private on Linux; elsewhere use a
        // directory under the temp dir that the server creates as 0700.
        let base = env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                // SAFETY: getuid has no preconditions.
                #[allow(unsafe_code)]
                let uid = unsafe { libc::getuid() };
                env::temp_dir().join(format!("surtr-{uid}"))
            });
        let name = custom.unwrap_or_else(|| "serve".to_owned());
        Ok(base.join("surtr").join(format!("{name}.sock")))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, atomic::AtomicU64},
        time::Duration,
    };

    use super::*;

    fn handler(method: &str) -> Result<Value, String> {
        match method {
            "echo" => Ok(Value::String("hi".to_owned())),
            other => Err(format!("unknown method {other}")),
        }
    }

    // A real round trip over the platform transport.
    #[test]
    fn round_trip() {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: tests in this crate don't read the variable concurrently
        // with this write except through `endpoint`, called after it.
        #[allow(unsafe_code)]
        unsafe {
            env::set_var(
                "SURTR_IPC_ENDPOINT",
                format!("surtr-ipc-test-{}-{n}", std::process::id()),
            )
        };
        let path = endpoint().unwrap();
        let hello = Hello {
            protocol: PROTOCOL,
            version: "test".to_owned(),
            providers: vec!["mock".to_owned()],
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server_path = path.clone();
        let server = std::thread::spawn(move || {
            runtime.block_on(async move {
                serve(&server_path, hello, Arc::new(handler), async {
                    let _ = stopped.await;
                })
                .await
            })
        });
        // Wait for the endpoint to appear.
        let mut client = None;
        for _ in 0..50 {
            if let Ok(c) = Client::connect(&path) {
                client = Some(c);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut client = client.expect("connected");
        assert_eq!(client.hello().providers, ["mock"]);
        assert_eq!(client.call("echo").unwrap(), Value::String("hi".to_owned()));
        assert!(client.call("nope").unwrap_err().contains("unknown method"));
        drop(client);
        let _ = stop.send(());
        server.join().unwrap().unwrap();
    }
}
