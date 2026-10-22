//! The audit log and the rate limit (plan, section 6.1): every tool call is
//! appended to a JSON-lines file, and a client that calls too fast is told
//! to slow down.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::PathBuf,
    sync::Mutex,
    time::Instant,
};

use serde_json::{Map, Value, json};
use surtr_model::Timestamp;
use surtr_view::format;

/// The log is rotated (to `<name>.1.jsonl`) when it grows past this size.
const MAX_LOG_BYTES: u64 = 1 << 20;
/// Calls a client may make at once.
const BURST: f64 = 30.0;
/// Calls per second a client may sustain.
const PER_SECOND: f64 = 5.0;

/// One finished tool call.
#[derive(Debug)]
pub struct Entry<'a> {
    /// The client's name and version from the handshake.
    pub client: &'a str,
    /// The tool called.
    pub tool: &'a str,
    /// The arguments as sent.
    pub arguments: Option<&'a Map<String, Value>>,
    /// `ok`, `error` (the tool reported an error), `refused` (unknown tool or
    /// bad arguments) or `rate_limited`.
    pub outcome: &'static str,
    /// Size of the answer's text, in bytes.
    pub bytes: usize,
    /// How long the call took.
    pub ms: u128,
}

/// The audit log and the rate limiter of one server.
#[derive(Debug)]
pub struct Audit {
    path: Option<PathBuf>,
    tokens: Mutex<(f64, Instant)>,
}

impl Audit {
    /// Logs to `path` (no logging when `None`).
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            tokens: Mutex::new((BURST, Instant::now())),
        }
    }

    /// Takes one call from the budget; `false` when the client must wait.
    pub fn allow(&self) -> bool {
        let mut t = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let refill = now.duration_since(t.1).as_secs_f64() * PER_SECOND;
        *t = ((t.0 + refill).min(BURST), now);
        if t.0 >= 1.0 {
            t.0 -= 1.0;
            true
        } else {
            false
        }
    }

    /// Appends `entry`. Failures are logged, never returned: the audit log
    /// must not break the tool.
    pub fn record(&self, entry: &Entry<'_>) {
        let Some(path) = &self.path else { return };
        let line = json!({
            "at": format::iso_utc(Timestamp::now()),
            "client": entry.client,
            "tool": entry.tool,
            "arguments": entry.arguments,
            "outcome": entry.outcome,
            "bytes": entry.bytes,
            "ms": entry.ms,
        });
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
                fs::rename(path, path.with_extension("1.jsonl"))?;
            }
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(file, "{line}")
        };
        if let Err(e) = write() {
            tracing::warn!("MCP audit log {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_refuse() {
        let audit = Audit::new(None);
        let allowed = (0..100).filter(|_| audit.allow()).count();
        // The burst, plus whatever refilled while the loop ran.
        assert!((30..=31).contains(&allowed), "{allowed}");
    }

    #[test]
    fn appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("surtr-mcp-audit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("mcp-audit.jsonl");
        let audit = Audit::new(Some(path.clone()));
        let entry = Entry {
            client: "test 1.0",
            tool: "get_sensors",
            arguments: None,
            outcome: "ok",
            bytes: 10,
            ms: 1,
        };
        audit.record(&entry);
        audit.record(&entry);
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["tool"], "get_sensors");
        assert_eq!(first["client"], "test 1.0");

        fs::write(&path, vec![b' '; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        audit.record(&entry);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert!(path.with_extension("1.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
