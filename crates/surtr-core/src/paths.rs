//! Per-user locations for Surtr's files, and redaction of the home directory
//! from text that may be shared (logs, diagnostic bundles).
//!
//! `SURTR_DATA_DIR` and `SURTR_LOG_DIR` override the defaults (used by tests
//! and for portable setups).

use std::{env, path::PathBuf};

fn var(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The user's home directory.
pub fn home() -> Option<PathBuf> {
    if cfg!(windows) {
        var("USERPROFILE")
    } else {
        var("HOME")
    }
}

/// Surtr's data directory: `%LOCALAPPDATA%\surtr`,
/// `~/Library/Application Support/surtr`, or `$XDG_DATA_HOME/surtr`
/// (`~/.local/share/surtr`).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = var("SURTR_DATA_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else {
        var("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local/share")))?
    };
    Some(base.join("surtr"))
}

/// Surtr's log directory: `%LOCALAPPDATA%\surtr\logs`,
/// `~/Library/Logs/surtr`, or `$XDG_STATE_HOME/surtr/logs`
/// (`~/.local/state/surtr/logs`).
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = var("SURTR_LOG_DIR") {
        return Some(dir);
    }
    if cfg!(windows) {
        Some(var("LOCALAPPDATA")?.join("surtr").join("logs"))
    } else if cfg!(target_os = "macos") {
        Some(home()?.join("Library/Logs/surtr"))
    } else {
        let base = var("XDG_STATE_HOME").or_else(|| home().map(|h| h.join(".local/state")))?;
        Some(base.join("surtr").join("logs"))
    }
}

/// The default sensor history database.
pub fn history_db() -> Option<PathBuf> {
    data_dir().map(|d| d.join("history.db"))
}

/// `text` with the home directory replaced by `~`, so user names in paths
/// don't end up in logs or bug reports.
pub fn redact(text: &str) -> String {
    let Some(home) = home().and_then(|h| h.to_str().map(str::to_owned)) else {
        return text.to_owned();
    };
    if home.len() < 3 {
        return text.to_owned();
    }
    let mut out = text.replace(&home, "~");
    // The same path written with the other separator, and JSON-escaped.
    out = out.replace(&home.replace('\\', "/"), "~");
    out.replace(&home.replace('\\', "\\\\"), "~")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_home() {
        let Some(home) = home().and_then(|h| h.to_str().map(str::to_owned)) else {
            return;
        };
        let text = format!("db at {home}/x and {}", home.replace('\\', "\\\\"));
        let out = redact(&text);
        assert!(!out.contains(&home), "{out}");
        assert!(out.starts_with("db at ~"), "{out}");
    }

    #[test]
    fn locations_end_in_surtr() {
        if let Some(d) = data_dir() {
            assert!(d.ends_with("surtr") || env::var_os("SURTR_DATA_DIR").is_some());
        }
        if let Some(d) = history_db() {
            assert!(d.ends_with("history.db"));
        }
    }
}
