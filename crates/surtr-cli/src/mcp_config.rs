//! `surtr mcp config <client>`: the exact setup for an MCP client, with the
//! full path of this `surtr`, so the client finds it even when `surtr` is
//! not on the PATH it sees. Only prints; it never edits a client's files.

use std::path::Path;

use clap::ValueEnum;
use serde_json::json;

/// MCP clients `surtr mcp config` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Client {
    /// Claude Code (the `claude` CLI).
    ClaudeCode,
    /// Claude Desktop.
    ClaudeDesktop,
    /// Cursor.
    Cursor,
    /// Visual Studio Code.
    Vscode,
}

/// Where Claude Desktop keeps its settings on this platform.
fn claude_desktop_file() -> &'static str {
    if cfg!(windows) {
        r"%APPDATA%\Claude\claude_desktop_config.json"
    } else if cfg!(target_os = "macos") {
        "~/Library/Application Support/Claude/claude_desktop_config.json"
    } else {
        "the Claude Desktop config file (claude_desktop_config.json)"
    }
}

/// The setup text for `client`, running `exe mcp`.
pub fn text(client: Client, exe: &Path) -> String {
    let exe = exe.display().to_string();
    let server = json!({ "command": exe, "args": ["mcp"] });
    let pretty = |v: serde_json::Value| serde_json::to_string_pretty(&v).unwrap_or_default();
    match client {
        Client::ClaudeCode => format!(
            "Run this once (--scope user makes Surtr available in every project):\n\n\
             claude mcp add --scope user surtr -- \"{exe}\" mcp\n"
        ),
        Client::ClaudeDesktop => format!(
            "Add this to {}, merging it with any \"mcpServers\" already there, then \
             restart Claude Desktop:\n\n{}\n",
            claude_desktop_file(),
            pretty(json!({ "mcpServers": { "surtr": server } }))
        ),
        Client::Cursor => format!(
            "Add this to ~/.cursor/mcp.json (all projects) or .cursor/mcp.json (one \
             project), merging it with any \"mcpServers\" already there:\n\n{}\n",
            pretty(json!({ "mcpServers": { "surtr": server } }))
        ),
        Client::Vscode => format!(
            "Add this to .vscode/mcp.json in your workspace, or to your user profile \
             with the \"MCP: Open User Configuration\" command, merging it with any \
             \"servers\" already there:\n\n{}\n",
            pretty(json!({
                "servers": { "surtr": { "type": "stdio", "command": exe, "args": ["mcp"] } }
            }))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The JSON block after the first blank line.
    fn block(text: &str) -> serde_json::Value {
        let (_, json) = text.split_once("\n\n").unwrap();
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn configs_use_the_full_path() {
        // A path with a space and backslashes must survive JSON escaping.
        let exe = Path::new(r"C:\Program Files\surtr\surtr.exe");
        let v = block(&text(Client::ClaudeDesktop, exe));
        assert_eq!(
            v["mcpServers"]["surtr"]["command"],
            exe.display().to_string()
        );
        assert_eq!(v["mcpServers"]["surtr"]["args"][0], "mcp");
        let v = block(&text(Client::Cursor, exe));
        assert_eq!(v["mcpServers"]["surtr"]["args"][0], "mcp");
        let v = block(&text(Client::Vscode, exe));
        assert_eq!(v["servers"]["surtr"]["type"], "stdio");
        let code = text(Client::ClaudeCode, exe);
        assert!(
            code.contains(r#"-- "C:\Program Files\surtr\surtr.exe" mcp"#),
            "{code}"
        );
    }
}
