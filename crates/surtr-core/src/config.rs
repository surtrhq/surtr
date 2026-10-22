//! The settings file (`config.toml` in [`crate::paths::config_dir`]).
//!
//! Every setting is optional; a missing file means defaults. Command-line
//! flags override the file. Unknown keys produce warnings, never errors, so a
//! file written for a newer Surtr still loads.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

/// The commented file `surtr config init` writes.
pub const TEMPLATE: &str = r#"# Surtr settings. Every setting is optional; command-line flags override them.

[general]
# Milliseconds between sensor reads (surtr sensors, watch, tui, record).
# interval_ms = 1000
# TUI colors: "dark" or "light".
# theme = "dark"

[history]
# Delete samples older than this (for example "7d", "12h"); "0" keeps everything.
# retain = "7d"
# Database file; default is history.db in Surtr's data directory.
# path = "C:/path/to/history.db"

[alerts]
# Desktop notifications for alerts that surtr serve raises.
# notify = true
"#;

/// General settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct General {
    /// Milliseconds between sensor reads.
    pub interval_ms: Option<u64>,
    /// TUI theme name: `dark` or `light`.
    pub theme: Option<String>,
}

/// History settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Retention, as a duration such as `7d`.
    pub retain: Option<String>,
    /// Database file.
    pub path: Option<String>,
}

/// Alert settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alerts {
    /// Show desktop notifications for alerts raised by `surtr serve`.
    pub notify: Option<bool>,
}

impl Alerts {
    /// Whether to notify (default: yes).
    pub fn notify(&self) -> bool {
        self.notify.unwrap_or(true)
    }
}

/// All settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `[general]`.
    #[serde(default)]
    pub general: General,
    /// `[history]`.
    #[serde(default)]
    pub history: History,
    /// `[alerts]`.
    #[serde(default)]
    pub alerts: Alerts,
}

/// Settings plus warnings about the file (unknown keys).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    /// The settings.
    pub config: Config,
    /// Human-readable problems that didn't stop loading.
    pub warnings: Vec<String>,
}

/// Keys Surtr knows, as `section.key`.
const KNOWN: [&str; 5] = [
    "alerts.notify",
    "general.interval_ms",
    "general.theme",
    "history.retain",
    "history.path",
];

/// Parses settings text. Wrong types are errors; unknown keys are warnings.
pub fn parse(text: &str) -> Result<Loaded, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut warnings = Vec::new();
    for (section, value) in &table {
        match value.as_table() {
            Some(keys) => {
                for key in keys.keys() {
                    let name = format!("{section}.{key}");
                    if !KNOWN.contains(&name.as_str()) {
                        warnings.push(format!("unknown setting {name}"));
                    }
                }
            }
            None => warnings.push(format!("unknown setting {section}")),
        }
    }
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(Loaded { config, warnings })
}

/// Loads the file at `path`; a missing file means defaults.
pub fn load(path: &Path) -> Result<Loaded, String> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Loaded::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Config {
    /// The effective settings as TOML, defaults filled in.
    pub fn effective(&self) -> String {
        let shown = Config {
            general: General {
                interval_ms: Some(self.general.interval_ms.unwrap_or(1000)),
                theme: Some(
                    self.general
                        .theme
                        .clone()
                        .unwrap_or_else(|| "dark".to_owned()),
                ),
            },
            history: History {
                retain: Some(
                    self.history
                        .retain
                        .clone()
                        .unwrap_or_else(|| "7d".to_owned()),
                ),
                path: self.history.path.clone(),
            },
            alerts: Alerts {
                notify: Some(self.alerts.notify()),
            },
        };
        toml::to_string_pretty(&shown).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let loaded = parse(TEMPLATE).unwrap();
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn values_and_unknown_keys() {
        let loaded = parse(
            "[general]\ninterval_ms = 500\ncolour = \"red\"\n[history]\nretain = \"2d\"\n[future]\nx = 1\n",
        )
        .unwrap();
        assert_eq!(loaded.config.general.interval_ms, Some(500));
        assert_eq!(loaded.config.history.retain.as_deref(), Some("2d"));
        // Sorted: toml tables keep keys in order.
        assert_eq!(
            loaded.warnings,
            ["unknown setting future.x", "unknown setting general.colour"]
        );
    }

    #[test]
    fn wrong_types_are_errors() {
        assert!(parse("[general]\ninterval_ms = \"fast\"\n").is_err());
        assert!(parse("not toml").is_err());
    }

    #[test]
    fn missing_file_means_defaults() {
        let loaded = load(Path::new("/nonexistent/surtr/config.toml")).unwrap();
        assert_eq!(loaded, Loaded::default());
    }

    #[test]
    fn effective_shows_defaults() {
        let text = Config::default().effective();
        assert!(text.contains("interval_ms = 1000"), "{text}");
        assert!(text.contains("theme = \"dark\""), "{text}");
        assert!(text.contains("retain = \"7d\""), "{text}");
    }
}
