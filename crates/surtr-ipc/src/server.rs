//! The daemon side: accept owner-only connections and answer requests.

use std::{future::Future, io, path::Path, sync::Arc};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::{Hello, MAX_REQUEST, Request, Response};

/// Answers one method call.
pub type Handler = Arc<dyn Fn(&str) -> Result<Value, String> + Send + Sync>;

/// Serves `endpoint` until `shutdown` completes. `hello` answers the
/// handshake; every other method goes to `handler`.
pub async fn serve(
    endpoint: &Path,
    hello: Hello,
    handler: Handler,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let hello = serde_json::to_value(hello).map_err(io::Error::other)?;
    platform::serve(endpoint, hello, handler, shutdown).await
}

/// Handles one connection: one request per line, one response per line.
async fn conversation<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    hello: Value,
    handler: Handler,
) {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        // Bounded read: a request longer than MAX_REQUEST ends the connection.
        let mut limited = (&mut lines).take(MAX_REQUEST as u64 + 1);
        match limited.read_line(&mut line).await {
            Ok(0) => return,
            Ok(n) if n > MAX_REQUEST => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let result = if req.method == "hello" {
                    Ok(hello.clone())
                } else {
                    handler(&req.method)
                };
                match result {
                    Ok(v) => Response {
                        id: req.id,
                        ok: Some(v),
                        error: None,
                    },
                    Err(e) => Response {
                        id: req.id,
                        ok: None,
                        error: Some(e),
                    },
                }
            }
            Err(e) => Response {
                id: 0,
                ok: None,
                error: Some(format!("bad request: {e}")),
            },
        };
        let Ok(mut text) = serde_json::to_string(&response) else {
            return;
        };
        text.push('\n');
        if write.write_all(text.as_bytes()).await.is_err() {
            return;
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::{
        fs::{self, DirBuilder, Permissions},
        future::Future,
        io,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        path::Path,
    };

    use serde_json::Value;
    use tokio::net::UnixListener;

    use super::{Handler, conversation};

    pub async fn serve(
        endpoint: &Path,
        hello: Value,
        handler: Handler,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        // SAFETY: getuid has no preconditions.
        #[allow(unsafe_code)]
        let me = unsafe { libc::getuid() };
        if let Some(dir) = endpoint.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            // Another user could have created the directories first (the
            // temp-dir fallback is guessable): refuse rather than share them.
            for d in [Some(dir), dir.parent()].into_iter().flatten() {
                if fs::symlink_metadata(d)?.uid() != me {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("{} belongs to another user", d.display()),
                    ));
                }
            }
            fs::set_permissions(dir, Permissions::from_mode(0o700))?;
        }
        // A socket file left by a crashed daemon: remove it only if nothing
        // answers on it.
        if endpoint.exists() {
            if std::os::unix::net::UnixStream::connect(endpoint).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another surtr serve is already listening",
                ));
            }
            fs::remove_file(endpoint)?;
        }
        let listener = UnixListener::bind(endpoint)?;
        fs::set_permissions(endpoint, Permissions::from_mode(0o600))?;
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    // Only the daemon's own user may talk to it.
                    match stream.peer_cred() {
                        Ok(cred) if cred.uid() == me => {}
                        _ => {
                            tracing::warn!("refused an IPC connection from another user");
                            continue;
                        }
                    }
                    tokio::spawn(conversation(stream, hello.clone(), handler.clone()));
                }
            }
        }
        let _ = fs::remove_file(endpoint);
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use std::{future::Future, io, path::Path};

    use serde_json::Value;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    use super::{Handler, conversation};
    use crate::windows::OwnerOnly;

    fn create(endpoint: &Path, security: &OwnerOnly, first: bool) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: the security attributes stay alive in `security` for the
        // whole time the server runs.
        #[allow(unsafe_code)]
        unsafe {
            options.create_with_security_attributes_raw(endpoint, security.attributes())
        }
    }

    pub async fn serve(
        endpoint: &Path,
        hello: Value,
        handler: Handler,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let security = OwnerOnly::new()?;
        // first_pipe_instance: fail if anyone else already owns this name.
        let mut server = create(endpoint, &security, true).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("can't create the IPC pipe (is another surtr serve running?): {e}"),
            )
        })?;
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                connected = server.connect() => {
                    if connected.is_err() {
                        continue;
                    }
                    let next = create(endpoint, &security, false)?;
                    let stream = std::mem::replace(&mut server, next);
                    tokio::spawn(conversation(stream, hello.clone(), handler.clone()));
                }
            }
        }
        Ok(())
    }
}
