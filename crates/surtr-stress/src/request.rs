//! Stress-run requests from AI agents (plan, section 6.1, `run` scope).
//!
//! An agent never starts a run. It files a request, and a person confirms it
//! with `surtr runs confirm <id>` in a terminal on the machine; the run then
//! happens in that terminal, under the usual limits. Requests left alone
//! expire. Each request is a small JSON file in `runs/requests/`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Version of the request format.
pub const SCHEMA: u32 = 1;
/// How long a request waits for a person, in milliseconds.
pub const EXPIRES_MS: u64 = 10 * 60_000;

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// Waiting for a person to confirm or deny it.
    Pending,
    /// A person said no.
    Denied,
    /// Nobody answered in time.
    Expired,
    /// Confirmed; [`Request::run`] is the run.
    Started,
}

/// A request to run a stress test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Format version, [`SCHEMA`].
    pub schema: u32,
    /// Short code the person types, for example `4f2a9c`.
    pub id: String,
    /// Who asked: the MCP client's name and version.
    pub client: String,
    /// When, in milliseconds since the Unix epoch.
    pub created_ms: u64,
    /// Workload, for example `cpu`.
    pub workload: String,
    /// Requested length, in milliseconds.
    pub duration_ms: u64,
    /// Requested threads; `None` means one per logical CPU.
    pub threads: Option<u32>,
    /// Where it stands.
    pub state: RequestState,
    /// The run it became, once started.
    pub run: Option<String>,
}

/// The requests directory inside the runs directory.
pub fn dir(runs: &Path) -> PathBuf {
    runs.join("requests")
}

fn path(runs: &Path, id: &str) -> PathBuf {
    dir(runs).join(format!("{id}.json"))
}

/// The file whose presence asks a running run to stop.
pub fn stop_path(runs: &Path, run: &str) -> PathBuf {
    runs.join(format!("{run}.stop"))
}

/// Stores `request`, replacing an earlier version.
pub fn save(runs: &Path, request: &Request) -> io::Result<()> {
    fs::create_dir_all(dir(runs))?;
    let bytes = serde_json::to_vec_pretty(request).map_err(io::Error::other)?;
    fs::write(path(runs, &request.id), bytes)
}

/// Files a new request at `now_ms`.
pub fn create(
    runs: &Path,
    client: &str,
    workload: &str,
    duration_ms: u64,
    threads: Option<u32>,
    now_ms: u64,
) -> io::Result<Request> {
    // A short code a person can type; unique among the stored requests.
    let mut seed = now_ms ^ (u64::from(std::process::id()) << 20);
    let id = loop {
        seed = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(17)
            .wrapping_add(1);
        let id = format!("{:06x}", seed & 0xFF_FFFF);
        if !path(runs, &id).exists() {
            break id;
        }
    };
    let request = Request {
        schema: SCHEMA,
        id,
        client: client.to_owned(),
        created_ms: now_ms,
        workload: workload.to_owned(),
        duration_ms,
        threads,
        state: RequestState::Pending,
        run: None,
    };
    save(runs, &request)?;
    Ok(request)
}

/// The request `id` as of `now_ms`: a pending one past its time becomes
/// expired (and is stored that way).
pub fn load(runs: &Path, id: &str, now_ms: u64) -> io::Result<Option<Request>> {
    // Ids are hex codes; anything else can't name a file of ours.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let bytes = match fs::read(path(runs, id)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let Ok(mut request) = serde_json::from_slice::<Request>(&bytes) else {
        return Ok(None);
    };
    if request.state == RequestState::Pending
        && now_ms.saturating_sub(request.created_ms) > EXPIRES_MS
    {
        request.state = RequestState::Expired;
        save(runs, &request)?;
    }
    Ok(Some(request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_live_and_expire() {
        let runs = std::env::temp_dir().join(format!("surtr-requests-{}", std::process::id()));
        let _ = fs::remove_dir_all(&runs);
        let r = create(&runs, "agent 1.0", "cpu", 300_000, None, 1_000_000).unwrap();
        assert_eq!(r.id.len(), 6);
        let again = create(&runs, "agent 1.0", "cpu", 300_000, None, 1_000_000).unwrap();
        assert_ne!(r.id, again.id);

        let got = load(&runs, &r.id, 1_000_000 + EXPIRES_MS).unwrap().unwrap();
        assert_eq!(got.state, RequestState::Pending);
        let got = load(&runs, &r.id, 1_000_001 + EXPIRES_MS).unwrap().unwrap();
        assert_eq!(got.state, RequestState::Expired);
        // Stored as expired: it stays expired even if asked about earlier.
        let got = load(&runs, &r.id, 0).unwrap().unwrap();
        assert_eq!(got.state, RequestState::Expired);

        assert!(load(&runs, "../../etc/passwd", 0).unwrap().is_none());
        assert!(load(&runs, "abcdef", 0).unwrap().is_none());
        let _ = fs::remove_dir_all(&runs);
    }
}
