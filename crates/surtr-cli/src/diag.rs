//! `surtr diag`: a redacted diagnostic bundle for bug reports (plan,
//! section 8.1). Everything is built in memory, shown to the user, and only
//! written after they agree. Nothing is ever uploaded.

use std::{
    fs,
    io::{self, Write as _},
    path::Path,
    thread,
    time::Duration,
};

use serde_json::json;
use surtr_core::{Hub, paths};
use surtr_model::{Feature, Support, Timestamp};
use surtr_view::{Level, format};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::history::iso_utc;

/// Version of the bundle layout (`manifest.json`).
const BUNDLE_VERSION: u32 = 1;
/// Lines kept from each log file, newest last.
const LOG_LINES: usize = 2000;

/// A diagnostic bundle: a Markdown summary and the files of the zip.
#[derive(Debug)]
pub struct Bundle {
    /// `summary.md`, ready to paste into an issue.
    pub summary: String,
    /// Files in the zip, by name, all redacted.
    pub files: Vec<(String, Vec<u8>)>,
}

/// A file-name-safe UTC time stamp such as `20261021-224850`.
pub fn stamp(t: Timestamp) -> String {
    iso_utc(t).replace(['-', ':', 'Z'], "").replace('T', "-")
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// Text files in `dir` whose names match, oldest first by name.
fn files(dir: &Path, prefix: &str, suffix: &str, keep: usize) -> Vec<(String, String)> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
        .collect();
    names.sort();
    let skip = names.len().saturating_sub(keep);
    names
        .into_iter()
        .skip(skip)
        .filter_map(|n| {
            let text = fs::read_to_string(dir.join(&n)).ok()?;
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(LOG_LINES)..].join("\n");
            Some((n, paths::redact(&tail)))
        })
        .collect()
}

/// Collects everything for a bundle. Takes about half a second (rate
/// sensors need two samples).
pub fn build(hub: &Hub) -> Bundle {
    let system = hub.system_info();
    let capabilities = hub.capabilities();
    hub.read();
    thread::sleep(Duration::from_millis(500));
    let samples = hub.read();
    let throttle = hub.throttle_reasons();
    let status = surtr_view::status(&system, &samples, &throttle, &capabilities);
    let providers = hub.provider_ids().join(", ");

    let (logs, crashes) = match paths::log_dir() {
        Some(dir) => (
            files(&dir, "surtr", ".log", 3),
            files(&dir, "crash-", ".txt", 5),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let warnings: Vec<&str> = logs
        .iter()
        .flat_map(|(_, text)| text.lines())
        .filter(|l| l.contains(" WARN ") || l.contains(" ERROR "))
        .collect();
    let warnings = &warnings[warnings.len().saturating_sub(20)..];

    // The summary: what a maintainer needs first, with no identifiers.
    let mut s = String::from("## Surtr diagnostic summary\n\n| | |\n|---|---|\n");
    let mut row = |k: &str, v: String| s.push_str(&format!("| {k} | {} |\n", cell(&v)));
    row(
        "Surtr",
        format!(
            "{} ({}, {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
    let field = |f: &surtr_model::Field<String>| f.as_ref().map(|f| f.value.clone());
    let os: Vec<String> = [
        field(&system.os.name),
        field(&system.os.version),
        system
            .os
            .kernel
            .as_ref()
            .map(|k| format!("kernel {}", k.value)),
        system.os.arch.as_ref().map(|a| format::arch(&a.value)),
    ]
    .into_iter()
    .flatten()
    .collect();
    row("OS", os.join(", "));
    for (i, cpu) in system.cpus.iter().enumerate() {
        let name = field(&cpu.brand).unwrap_or_else(|| format::MISSING.to_owned());
        let cores = cpu.physical_cores.as_ref().map(|c| c.value);
        let threads = cpu.logical_cores.as_ref().map(|c| c.value);
        let detail = match (cores, threads) {
            (Some(c), Some(t)) => format!(" ({c} cores, {t} threads)"),
            (None, Some(t)) => format!(" ({t} threads)"),
            _ => String::new(),
        };
        row(&format!("CPU {i}"), format!("{name}{detail}"));
    }
    row(
        "Memory",
        system
            .memory
            .total_bytes
            .as_ref()
            .map_or(format::MISSING.to_owned(), |m| format::bytes(m.value)),
    );
    for (i, gpu) in system.gpus.iter().enumerate() {
        let name = field(&gpu.name).unwrap_or_else(|| format::MISSING.to_owned());
        let driver = field(&gpu.driver_version)
            .map(|d| format!(", driver {d}"))
            .unwrap_or_default();
        row(
            &format!("GPU {i}"),
            format!("{name} ({}){driver}", gpu.device),
        );
    }
    row("Providers", providers.clone());

    let level = match status.level {
        Level::Ok => "OK",
        Level::Warn => "WARNING",
        Level::Critical => "CRITICAL",
    };
    s.push_str(&format!("\n**Health: {level}**\n"));
    for f in &status.findings {
        let tag = match f.level {
            Some(Level::Critical) => "critical",
            Some(Level::Warn) => "warning",
            Some(Level::Ok) | None => "note",
        };
        s.push_str(&format!("- {tag}: {}\n", f.message));
    }

    let missing: Vec<String> = Feature::ALL
        .iter()
        .filter_map(|&f| match capabilities.support(f) {
            Some(Support::Available) => None,
            other => Some(format!(
                "- {}: {}",
                format::feature(f),
                format::support(other).0
            )),
        })
        .collect();
    if !missing.is_empty() {
        s.push_str("\n**Not available on this machine**\n");
        s.push_str(&missing.join("\n"));
        s.push('\n');
    }

    s.push_str("\n**Recent log warnings and errors**\n\n```\n");
    if warnings.is_empty() {
        s.push_str("(none)\n");
    }
    for w in warnings {
        s.push_str(w);
        s.push('\n');
    }
    s.push_str("```\n");
    if !crashes.is_empty() {
        s.push_str(&format!(
            "\n{} crash report(s) included in the bundle.\n",
            crashes.len()
        ));
    }
    let summary = paths::redact(&s);

    let pretty = |v: serde_json::Value| {
        paths::redact(&serde_json::to_string_pretty(&v).unwrap_or_default()).into_bytes()
    };
    let mut files = vec![
        ("summary.md".to_owned(), summary.clone().into_bytes()),
        ("system.json".to_owned(), pretty(json!(system))),
        ("capabilities.json".to_owned(), pretty(json!(capabilities))),
        ("sensors.json".to_owned(), pretty(json!(samples))),
        ("throttle.json".to_owned(), pretty(json!(throttle))),
    ];
    for (name, text) in logs {
        files.push((format!("logs/{name}"), text.into_bytes()));
    }
    for (name, text) in crashes {
        files.push((format!("crashes/{name}"), text.into_bytes()));
    }
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    let manifest = pretty(json!({
        "bundle_version": BUNDLE_VERSION,
        "surtr_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "created": iso_utc(Timestamp::now()),
        "providers": providers,
        "files": names,
    }));
    files.insert(0, ("manifest.json".to_owned(), manifest));
    Bundle { summary, files }
}

/// Writes the bundle as a zip file.
pub fn write(bundle: &Bundle, path: &Path) -> io::Result<()> {
    let mut zip = ZipWriter::new(fs::File::create(path)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in &bundle.files {
        zip.start_file(name.as_str(), options)
            .map_err(io::Error::other)?;
        zip.write_all(bytes)?;
    }
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_file_safe() {
        assert_eq!(stamp(Timestamp(1_767_225_600_000)), "20260101-000000");
    }

    #[test]
    fn table_cells_escape_pipes() {
        assert_eq!(cell("a|b"), "a\\|b");
    }
}
