//! The bearer token for MCP over HTTP: 32 random bytes, hex-encoded, kept
//! in a file only the user can read.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// Where the token lives: `mcp-token` next to the history database.
pub fn path(db: &Path) -> PathBuf {
    db.with_file_name("mcp-token")
}

/// The token in `file`, created on first use.
pub fn load_or_create(file: &Path) -> io::Result<String> {
    match fs::read_to_string(file) {
        Ok(text) if text.trim().len() == 64 => Ok(text.trim().to_owned()),
        Ok(_) => create(file),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create(file),
        Err(e) => Err(e),
    }
}

/// Replaces the token in `file` with a new one; old clients stop working.
pub fn create(file: &Path) -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    // Write a fresh file, then move it into place, so a reader never sees
    // half a token. On Unix it is created 0600; on Windows the per-user
    // data directory is already private to the user.
    let tmp = file.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(&tmp)?;
    out.write_all(token.as_bytes())?;
    out.sync_all()?;
    drop(out);
    fs::rename(&tmp, file)?;
    Ok(token)
}

/// Compares in constant time, so response timing reveals nothing about how
/// much of a guess was right.
pub fn matches(expected: &str, given: &str) -> bool {
    expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_once_and_rotates() {
        let dir = std::env::temp_dir().join(format!("surtr-mcp-token-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = path(&dir.join("history.db"));
        let first = load_or_create(&file).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(load_or_create(&file).unwrap(), first);
        let second = create(&file).unwrap();
        assert_ne!(second, first);
        assert_eq!(load_or_create(&file).unwrap(), second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compares_whole_tokens() {
        assert!(matches("abc", "abc"));
        assert!(!matches("abc", "abd"));
        assert!(!matches("abc", "ab"));
        assert!(!matches("abc", ""));
    }
}
