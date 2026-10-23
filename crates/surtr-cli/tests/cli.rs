//! Runs the real `surtr` binary.

use std::process::Command;

use serde_json::Value;

/// The surtr binary with logs going to a test directory, never the user's.
fn surtr_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_surtr"));
    cmd.env("SURTR_LOG_DIR", test_log_dir());
    // Never read the user's settings; config tests point this elsewhere.
    cmd.env(
        "SURTR_CONFIG_DIR",
        std::env::temp_dir().join(format!("surtr-cli-test-config-{}", std::process::id())),
    );
    // Never talk to the user's surtr serve, nor to another test's.
    cmd.env("SURTR_IPC_ENDPOINT", unique_endpoint());
    // Runs and other data never land in the user's own data directory.
    cmd.env(
        "SURTR_DATA_DIR",
        std::env::temp_dir().join(format!("surtr-cli-test-data-{}", std::process::id())),
    );
    cmd
}

/// An IPC endpoint name no other command in this run uses.
fn unique_endpoint() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "surtr-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn test_log_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("surtr-cli-test-logs-{}", std::process::id()))
}

fn surtr(args: &[&str]) -> (bool, String) {
    let output = surtr_command()
        .args(args)
        .output()
        .expect("failed to run surtr");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("stdout is not UTF-8"),
    )
}

fn json(args: &[&str]) -> Value {
    let (ok, out) = surtr(args);
    assert!(ok, "surtr {args:?} failed");
    serde_json::from_str(&out).expect("output is not JSON")
}

#[test]
fn mock_info_json() {
    let v = json(&["--mock", "--json"]);
    assert_eq!(v["providers"][0], "mock");
    assert_eq!(
        v["system"]["cpus"][0]["brand"]["value"],
        "Mock 8-Core Processor"
    );
    assert_eq!(
        v["system"]["cpus"][0]["brand"]["source"]["provider"],
        "mock"
    );
}

#[test]
fn mock_caps_json() {
    let v = json(&["caps", "--mock", "--json"]);
    let caps = v["capabilities"].as_array().unwrap();
    assert!(
        caps.iter()
            .any(|c| c["feature"] == "cpu_identity" && c["support"]["status"] == "available")
    );
}

#[test]
fn mock_info_text() {
    let (ok, out) = surtr(&["info", "--mock"]);
    assert!(ok);
    assert!(out.contains("Mock 8-Core Processor"));
}

// The M1 exit criterion: `surtr --json` describes this machine's OS, CPU and
// memory. Runs on every CI runner.
#[test]
fn native_info_json() {
    let v = json(&["--json"]);
    let system = &v["system"];
    assert!(system["os"]["name"]["value"].is_string(), "{v:#}");
    assert!(
        system["cpus"][0]["logical_cores"]["value"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "{v:#}"
    );
    assert!(
        system["memory"]["total_bytes"]["value"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "{v:#}"
    );
}

#[test]
fn mock_sensors_json() {
    let v = json(&["sensors", "--mock", "--json", "--interval", "0"]);
    let sensors = v["sensors"].as_array().unwrap();
    let temp = sensors
        .iter()
        .find(|s| s["id"] == "cpu0/package/temp")
        .expect("package temperature");
    assert_eq!(temp["unit"], "celsius");
    assert!(temp["value"].is_f64());
}

// On Windows, Linux and macOS the total CPU load must be readable without a driver.
// The OS may mark a single sample invalid (Surtr then leaves it out), so a
// few attempts are allowed; a load that never appears still fails.
#[test]
fn native_sensors_json() {
    if !cfg!(any(windows, target_os = "linux", target_os = "macos")) {
        return;
    }
    let mut last = Value::Null;
    for _ in 0..3 {
        let v = json(&["sensors", "--json", "--interval", "300"]);
        let sensors = v["sensors"].as_array().unwrap();
        let load = sensors.iter().find(|s| s["id"] == "cpu/load");
        let load = load.expect("cpu/load sensor missing")["value"].as_f64();
        if load.is_some_and(|l| (0.0..=100.0).contains(&l)) {
            return;
        }
        last = v;
    }
    panic!("no valid total CPU load in 3 attempts: {last:#}");
}

#[test]
fn mock_gpu_procs_and_throttle_json() {
    let v = json(&["gpu", "procs", "--mock", "--json"]);
    assert_eq!(v["processes"][0]["pid"], 4242);
    assert_eq!(v["processes"][0]["kind"], "compute");
    let v = json(&["gpu", "throttle", "--mock", "--json"]);
    assert_eq!(v["throttle"][0]["reasons"][0], "power_cap");
}

// Both commands must work on every machine, with or without a GPU backend.
#[test]
fn native_gpu_commands_run() {
    json(&["gpu", "procs", "--json"]);
    json(&["gpu", "throttle", "--json"]);
    let caps = json(&["caps", "--json"]);
    let gpu = caps["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["feature"] == "gpu_identity")
        .expect("gpu_identity is always reported");
    assert!(gpu["support"]["status"].is_string(), "{caps:#}");
}

#[test]
fn record_then_history() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();

    // --retain 0: the mock's clock starts at 2026-01-01, which the default
    // 7-day retention would prune.
    let v = json(&[
        "record",
        "--mock",
        "--json",
        "--count",
        "3",
        "--retain",
        "0",
        "--interval",
        "0",
        "--db",
        db,
    ]);
    assert_eq!(v["samples"], 3);
    assert_eq!(v["values"], 3 * 8);

    let v = json(&["history", "--json", "--db", db]);
    assert_eq!(v["sensors"].as_array().unwrap().len(), 8);

    // The mock's clock starts at 2026-01-01, so look far enough back.
    let v = json(&[
        "history",
        "cpu0/package/temp",
        "--since",
        "1000d",
        "--json",
        "--db",
        db,
    ]);
    assert_eq!(v["points"].as_array().unwrap().len(), 3);
    let v = json(&[
        "history",
        "cpu0/package/temp",
        "--since",
        "1000d",
        "--bucket",
        "1h",
        "--json",
        "--db",
        db,
    ]);
    assert_eq!(v["points"][0]["count"], 3);

    let (ok, _) = surtr(&["history", "no/such/sensor", "--db", db]);
    assert!(!ok);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs surtr and returns (exit code, stdout).
fn surtr_code(args: &[&str]) -> (Option<i32>, String) {
    let output = surtr_command()
        .args(args)
        .output()
        .expect("failed to run surtr");
    (
        output.status.code(),
        String::from_utf8(output.stdout).expect("stdout is not UTF-8"),
    )
}

// Text output of every command against the mock, reviewed as snapshots
// (issue #13). Piped output carries no color codes.
#[test]
fn mock_text_snapshots() {
    let cases: [(&str, &[&str]); 6] = [
        ("info", &["--mock", "info"]),
        ("caps", &["--mock", "caps"]),
        ("sensors", &["--mock", "sensors", "--interval", "0"]),
        ("gpu_procs", &["--mock", "gpu", "procs"]),
        ("gpu_throttle", &["--mock", "gpu", "throttle"]),
        ("status", &["--mock", "status", "--interval", "0"]),
    ];
    for (name, args) in cases {
        let (_, out) = surtr_code(args);
        assert!(
            !out.contains('\u{1b}'),
            "{name}: color codes in piped output"
        );
        insta::assert_snapshot!(name, out);
    }
}

#[test]
fn status_exit_codes() {
    // The mock GPU is held at its power limit: a warning.
    let (code, _) = surtr_code(&["--mock", "status", "--interval", "0"]);
    assert_eq!(code, Some(2));
    let (code, out) = surtr_code(&["--mock", "status", "--interval", "0", "--json"]);
    assert_eq!(code, Some(2));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["level"], "warm");
}

#[test]
fn completions_need_no_hardware() {
    let (code, out) = surtr_code(&["completions", "bash"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("surtr"));
}

// Piped output is not a terminal, so the TUI must refuse rather than hang.
#[test]
fn tui_refuses_without_a_terminal() {
    let output = surtr_command()
        .args(["--mock", "tui"])
        .output()
        .expect("failed to run surtr");
    assert_eq!(output.status.code(), Some(1));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("needs an interactive terminal"), "{err}");
}

#[test]
fn commands_write_a_log_file() {
    surtr(&["--mock", "info"]);
    let logs: Vec<String> = std::fs::read_dir(test_log_dir())
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("surtr") && n.ends_with(".log"))
        .collect();
    assert!(!logs.is_empty(), "no log file");
    let text = std::fs::read_to_string(test_log_dir().join(&logs[0])).unwrap();
    assert!(text.contains("surtr started"), "{text}");
}

#[test]
fn diag_summary_and_bundle() {
    let (ok, summary) = surtr(&["--mock", "diag", "--summary"]);
    assert!(ok);
    assert!(
        summary.starts_with("## Surtr diagnostic summary"),
        "{summary}"
    );
    assert!(summary.contains("Mock 8-Core Processor (8 cores, 16 threads)"));
    assert!(summary.contains("Mock GPU 16 GB (gpu0), driver 1.0"));
    assert!(summary.contains("**Health: WARNING**"));
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        assert!(
            !summary.contains(home.to_str().unwrap()),
            "home directory leaked"
        );
    }

    let dir = std::env::temp_dir().join(format!("surtr-cli-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("bundle.zip");
    let (ok, _) = surtr(&[
        "--mock",
        "diag",
        "--yes",
        "--out",
        zip_path.to_str().unwrap(),
    ]);
    assert!(ok);
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    for expected in [
        "manifest.json",
        "summary.md",
        "system.json",
        "capabilities.json",
        "sensors.json",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} missing: {names:?}"
        );
    }
    let mut md = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("summary.md").unwrap(), &mut md).unwrap();
    assert!(md.starts_with("## Surtr diagnostic summary"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Without a terminal to answer the prompt, diag must not write silently.
#[test]
fn diag_needs_yes_without_a_terminal() {
    let output = surtr_command()
        .args(["--mock", "diag"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));
}

#[test]
fn watch_streams_json_lines() {
    let (code, out) = surtr_code(&[
        "--mock",
        "watch",
        "temp",
        "--json",
        "--count",
        "3",
        "--interval",
        "0",
    ]);
    assert_eq!(code, Some(0));
    let lines: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect();
    assert_eq!(lines.len(), 3);
    let ids: Vec<&str> = lines[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["cpu0/package/temp", "gpu0/temp"]);
}

#[test]
fn watch_text_frames_when_piped() {
    let (code, out) = surtr_code(&["--mock", "watch", "--count", "2", "--interval", "0"]);
    assert_eq!(code, Some(0));
    assert_eq!(out.matches("surtr watch · every 0 ms").count(), 2);
    assert!(!out.contains('\u{1b}'), "no escape codes when piped");
    let (code, _) = surtr_code(&[
        "--mock",
        "watch",
        "nothing-matches",
        "--count",
        "1",
        "--interval",
        "0",
    ]);
    assert_eq!(code, Some(1));
}

#[test]
fn export_csv_and_json() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();
    // --retain 0: the mock's clock starts at 2026-01-01.
    let (ok, _) = surtr(&[
        "record",
        "--mock",
        "--count",
        "3",
        "--retain",
        "0",
        "--interval",
        "0",
        "--db",
        db,
    ]);
    assert!(ok);

    let (ok, csv) = surtr(&["export", "--since", "1000d", "--db", db]);
    assert!(ok);
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some("time,provider,sensor,device,label,unit,avg,min,max,count")
    );
    assert_eq!(lines.count(), 3 * 8);
    assert!(csv.contains(",mock,cpu0/package/temp,cpu0,Package temperature,celsius,"));

    let v = json(&[
        "export", "temp", "--since", "1000d", "--bucket", "1h", "--format", "json", "--db", db,
    ]);
    let series = v.as_array().unwrap();
    assert_eq!(series.len(), 2);
    assert_eq!(series[0]["points"][0]["count"], 3);

    let (ok, _) = surtr(&["export", "no-such-sensor", "--since", "1000d", "--db", db]);
    assert!(!ok);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs surtr with settings in `dir`; returns (exit code, stdout, stderr).
fn surtr_with_config(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let output = surtr_command()
        .env("SURTR_CONFIG_DIR", dir)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn config_init_show_and_path() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let (code, out, _) = surtr_with_config(&dir, &["config", "path"]);
    assert_eq!(code, Some(0));
    assert!(out.trim_end().ends_with("config.toml"), "{out}");

    let (code, out, _) = surtr_with_config(&dir, &["config", "show"]);
    assert_eq!(code, Some(0));
    assert!(
        out.contains("interval_ms = 1000") && out.contains("retain = \"7d\""),
        "{out}"
    );

    let (code, _, _) = surtr_with_config(&dir, &["config", "init"]);
    assert_eq!(code, Some(0));
    assert!(dir.join("config.toml").exists());
    let (code, _, err) = surtr_with_config(&dir, &["config", "init"]);
    assert_eq!(code, Some(1), "init must not overwrite");
    assert!(err.contains("already exists"), "{err}");

    std::fs::write(
        dir.join("config.toml"),
        "[general]\ninterval_ms = 250\ntheme = \"light\"\nbogus = 1\n",
    )
    .unwrap();
    let (code, out, err) = surtr_with_config(&dir, &["config", "show", "--json"]);
    assert_eq!(code, Some(0));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["general"]["interval_ms"], 250);
    assert!(err.contains("unknown setting general.bogus"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// A broken settings file is reported and ignored; commands still work.
#[test]
fn broken_config_is_ignored() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-badconfig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[general]\ninterval_ms = \"fast\"\n",
    )
    .unwrap();
    let (code, out, err) = surtr_with_config(&dir, &["--mock", "info"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("Mock 8-Core Processor"));
    assert!(err.contains("ignoring the settings file"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// The interval from the settings file is used when no flag is given.
#[test]
fn config_interval_applies() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-interval-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "[general]\ninterval_ms = 0\n").unwrap();
    let (code, out, _) = surtr_with_config(&dir, &["--mock", "watch", "--count", "1"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("every 0 ms"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plain HTTP/1.1 GET; returns the response body.
fn http_get(addr: &str, path: &str) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default())
}

#[test]
fn serve_records_and_exposes_metrics() {
    use std::io::{BufRead, BufReader};
    let dir = std::env::temp_dir().join(format!("surtr-cli-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();

    let mut child = surtr_command()
        .args([
            "--mock",
            "serve",
            "--metrics",
            "127.0.0.1:0",
            "--count",
            "30",
            "--interval",
            "100",
            "--retain",
            "0",
            "--db",
            db,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    let mut line = String::new();
    while stderr.read_line(&mut line).unwrap() > 0 {
        if let Some(rest) = line.trim().strip_prefix("Serving metrics on http://") {
            addr = rest.strip_suffix("/metrics").map(str::to_owned);
            break;
        }
        line.clear();
    }
    let addr = addr.expect("serve printed its metrics address");

    // The first read lands after one interval; retry until it shows up.
    let mut body = String::new();
    for _ in 0..30 {
        body = http_get(&addr, "/metrics").unwrap_or_default();
        if body.contains("surtr_temperature_celsius{") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        body.contains("# TYPE surtr_temperature_celsius gauge"),
        "{body}"
    );
    assert!(body.contains("surtr_gpu_throttle_active{device=\"gpu0\",reason=\"power_cap\"} 1"));

    // A second serve on the same database is refused while the first runs.
    let (ok, _) = surtr(&[
        "--mock",
        "serve",
        "--count",
        "1",
        "--interval",
        "0",
        "--db",
        db,
    ]);
    assert!(!ok, "a second serve must not start");

    assert!(child.wait().unwrap().success());
    let (ok, _) = surtr(&["history", "--db", db]);
    assert!(ok);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commands_read_through_a_running_serve() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-client-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let endpoint = unique_endpoint();

    let mut child = surtr_command()
        .env("SURTR_IPC_ENDPOINT", &endpoint)
        .args(["--mock", "serve", "--count", "40", "--interval", "100"])
        .args(["--retain", "0", "--db", db.to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Without --mock the command still sees mock data: it came from serve.
    let sensors = |extra: &[&str]| {
        let out = surtr_command()
            .env("SURTR_IPC_ENDPOINT", &endpoint)
            .args(["sensors", "--json", "--interval", "0"])
            .args(extra)
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null)
    };
    let has_mock = |v: &Value| {
        v["sensors"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["id"] == "cpu0/package/temp"))
    };
    let mut remote = Value::Null;
    for _ in 0..30 {
        remote = sensors(&[]);
        if has_mock(&remote) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(has_mock(&remote), "{remote}");
    // --embedded reads this machine's hardware instead.
    assert!(!has_mock(&sensors(&["--embedded"])));

    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn serve_refuses_remote_metrics_without_opt_in() {
    let (code, _) = surtr_code(&[
        "--mock",
        "serve",
        "--metrics",
        "0.0.0.0:0",
        "--count",
        "1",
        "--interval",
        "0",
    ]);
    assert_eq!(code, Some(1));
}

#[test]
fn alerts_rules_and_log() {
    let v = json(&["--mock", "alerts", "--rules", "--json"]);
    let rules = v["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 4, "two rules per mock temperature sensor");
    assert_eq!(rules[1]["severity"], "critical");

    let dir = std::env::temp_dir().join(format!("surtr-cli-alerts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("history.db");
    let (ok, out) = surtr(&["alerts", "--db", db.to_str().unwrap()]);
    assert!(ok);
    assert!(out.starts_with("No alerts recorded"), "{out}");

    let event = r#"{"at":1767225600000,"rule":"critical-temperature/mock/cpu0/package/temp","state":"fired","severity":"critical","provider":"mock","sensor":"cpu0/package/temp","device":"cpu0","label":"Package temperature","value":96.5}"#;
    std::fs::write(dir.join("alerts.jsonl"), format!("{event}\nnot json\n")).unwrap();
    let (ok, out) = surtr(&["alerts", "--db", db.to_str().unwrap()]);
    assert!(ok);
    assert_eq!(
        out,
        "2026-01-01T00:00:00Z  ALERT critical: cpu0 Package temperature at 96.5 (critical-temperature/mock/cpu0/package/temp)\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs `surtr <args>` (which include `mcp`) and sends `calls` (method, params) after the
/// handshake; returns the result of each call.
fn mcp_session(args: &[&str], calls: &[(&str, Value)]) -> Vec<Value> {
    mcp_session_with(&[], args, calls)
}

/// [`mcp_session`] with extra environment variables.
fn mcp_session_with(
    envs: &[(&str, &std::path::Path)],
    args: &[&str],
    calls: &[(&str, Value)],
) -> Vec<Value> {
    use std::io::{BufRead, BufReader, Write};
    let mut child = surtr_command()
        .envs(envs.iter().copied())
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut send = |v: Value| writeln!(stdin, "{v}").unwrap();
    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "surtr-test", "version": "0" }
        }
    }));
    send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    for (i, (method, params)) in calls.iter().enumerate() {
        send(serde_json::json!({
            "jsonrpc": "2.0", "id": i + 1, "method": method, "params": params
        }));
    }
    let mut results = vec![Value::Null; calls.len() + 1];
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut answered = 0;
    while answered < results.len() {
        let line = lines.next().expect("surtr mcp stopped early").unwrap();
        let v: Value = serde_json::from_str(&line).expect("stdout is only JSON-RPC");
        if let Some(id) = v["id"].as_u64() {
            results[id as usize] = v["result"].clone();
            answered += 1;
        }
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
    results.remove(0);
    results
}

/// The text of a tool result, parsed as JSON when it is JSON.
fn tool_text(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

#[test]
fn mcp_answers_read_tools() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-mcp-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let call = |name: &str, arguments: Value| {
        (
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )
    };
    let r = mcp_session(
        &["--mock", "mcp", "--db", db.to_str().unwrap()],
        &[
            ("tools/list", serde_json::json!({})),
            call(
                "get_sensors",
                serde_json::json!({ "kind": "temperature", "device": "cpu0" }),
            ),
            call("get_system_info", serde_json::json!({})),
            call("get_throttle_reasons", serde_json::json!({})),
            call("get_diagnostics", serde_json::json!({})),
        ],
    );
    let names: Vec<&str> = r[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for tool in [
        "get_system_info",
        "get_sensors",
        "get_capabilities",
        "get_gpu_processes",
        "get_throttle_reasons",
        "query_history",
        "get_diagnostics",
        "list_stress_tests",
        "list_runs",
        "get_run",
    ] {
        assert!(names.contains(&tool), "{tool} missing: {names:?}");
    }
    // Every tool says it only reads, so clients can approve it once.
    for tool in r[0]["tools"].as_array().unwrap() {
        let hints = &tool["annotations"];
        assert_eq!(hints["readOnlyHint"], true, "{tool}");
        assert_eq!(hints["openWorldHint"], false, "{tool}");
    }

    // Structured content and the text carry the same JSON object.
    assert!(r[1]["structuredContent"].is_object(), "{}", r[1]);
    assert_eq!(tool_text(&r[1]), r[1]["structuredContent"]);
    let sensors = tool_text(&r[1]);
    let sensors = sensors["sensors"].as_array().unwrap();
    assert!(!sensors.is_empty());
    assert!(
        sensors
            .iter()
            .all(|s| s["kind"] == "temperature" && s["device"] == "cpu0")
    );
    assert!(sensors[0]["text"].as_str().unwrap().ends_with("°C"));

    assert_eq!(tool_text(&r[2])["providers"][0], "mock");
    assert!(tool_text(&r[3])["throttle"].is_array());
    let summary = tool_text(&r[4]);
    assert!(
        summary
            .as_str()
            .unwrap()
            .contains("Surtr diagnostic summary"),
        "{summary}"
    );

    // Every tool call is in the audit log; tools/list is not a call.
    let audit = std::fs::read_to_string(dir.join("mcp-audit.jsonl")).unwrap();
    let entries: Vec<Value> = audit
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries.len(), 4, "{audit}");
    // Calls run concurrently, so entries come in the order they finished.
    let sensors = entries
        .iter()
        .find(|e| e["tool"] == "get_sensors")
        .expect("get_sensors logged");
    assert_eq!(sensors["client"], "surtr-test 0");
    assert_eq!(sensors["arguments"]["kind"], "temperature");
    assert!(entries.iter().all(|e| e["outcome"] == "ok"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_history_lists_recorded_sensors() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();
    let call = |arguments: Value| {
        (
            "tools/call",
            serde_json::json!({ "name": "query_history", "arguments": arguments }),
        )
    };

    // Nothing recorded yet: an error result, not a protocol error.
    let r = mcp_session(
        &["--mock", "mcp", "--db", db],
        &[call(serde_json::json!({}))],
    );
    assert_eq!(r[0]["isError"], true);

    let (ok, _) = surtr(&[
        "record",
        "--mock",
        "--count",
        "2",
        "--retain",
        "0",
        "--interval",
        "0",
        "--db",
        db,
    ]);
    assert!(ok);
    let r = mcp_session(
        &["--mock", "mcp", "--db", db],
        &[
            call(serde_json::json!({})),
            call(serde_json::json!({ "sensor": "cpu0/package/temp", "minutes": 5 })),
            call(serde_json::json!({ "sensor": "no/such/sensor" })),
        ],
    );
    let listed = tool_text(&r[0]);
    let temp = listed["sensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "cpu0/package/temp")
        .expect("recorded package temperature");
    assert_eq!(temp["samples"], 2);
    assert!(temp["first"].as_str().unwrap().starts_with("2026-01-01T"));

    let query = tool_text(&r[1]);
    assert_eq!(query["unit"], "celsius");
    assert_eq!(query["bucket_seconds"], 2);
    assert_eq!(r[2]["isError"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_resources_and_prompts() {
    let dir = std::env::temp_dir().join(format!("surtr-cli-mcp-res-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let r = mcp_session(
        &["--mock", "mcp", "--db", db.to_str().unwrap()],
        &[
            ("resources/list", serde_json::json!({})),
            (
                "resources/read",
                serde_json::json!({ "uri": "surtr://sensors/live" }),
            ),
            ("prompts/list", serde_json::json!({})),
            (
                "prompts/get",
                serde_json::json!({
                    "name": "diagnose_instability",
                    "arguments": { "symptom": "it freezes in games" }
                }),
            ),
        ],
    );
    let uris: Vec<&str> = r[0]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["uri"].as_str())
        .collect();
    assert_eq!(
        uris,
        [
            "surtr://system",
            "surtr://sensors/live",
            "surtr://capabilities"
        ]
    );

    let contents = &r[1]["contents"][0];
    assert_eq!(contents["mimeType"], "application/json");
    let sensors: Value = serde_json::from_str(contents["text"].as_str().unwrap()).unwrap();
    assert!(
        sensors["sensors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "cpu0/package/temp")
    );

    assert_eq!(r[2]["prompts"][0]["name"], "diagnose_instability");
    let text = r[3]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.starts_with("My computer is unstable: it freezes in games\n\n"));
    assert!(text.contains("get_throttle_reasons"));

    let audit = std::fs::read_to_string(dir.join("mcp-audit.jsonl")).unwrap();
    assert!(
        audit.contains("\"resource surtr://sensors/live\""),
        "{audit}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// POSTs JSON to `addr` and returns the raw response (status line, headers
/// and body). Event streams may stay open, so reading stops at `until`, at
/// end of stream, or after 5 seconds.
fn http_post(addr: &str, path: &str, headers: &[(&str, &str)], body: &str, until: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .unwrap();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut response = Vec::new();
    let mut buf = [0u8; 4096];
    while std::time::Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => {}
        }
        if !until.is_empty() && String::from_utf8_lossy(&response).contains(until) {
            break;
        }
    }
    String::from_utf8_lossy(&response).into_owned()
}

#[test]
fn serve_answers_mcp_over_http_with_a_token() {
    use std::io::{BufRead, BufReader};
    let dir = std::env::temp_dir().join(format!("surtr-cli-mcp-http-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();

    // Only loopback addresses are accepted.
    let (code, _) = surtr_code(&[
        "--mock",
        "serve",
        "--mcp",
        "0.0.0.0:0",
        "--count",
        "1",
        "--db",
        db,
    ]);
    assert_eq!(code, Some(1));

    let mut child = surtr_command()
        .args(["--mock", "serve", "--mcp", "127.0.0.1:0", "--count", "60"])
        .args(["--interval", "100", "--retain", "0", "--db", db])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    let mut line = String::new();
    while stderr.read_line(&mut line).unwrap() > 0 {
        if let Some(rest) = line.trim().strip_prefix("Serving MCP on http://") {
            addr = rest.split_once("/mcp").map(|(a, _)| a.to_owned());
            break;
        }
        line.clear();
    }
    let addr = addr.expect("serve printed its MCP address");
    let (ok, token) = surtr(&["token", "--db", db]);
    assert!(ok);
    let token = token.trim();
    assert_eq!(token.len(), 64);
    let bearer = format!("Bearer {token}");

    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "surtr-http-test", "version": "0" }
        }
    })
    .to_string();
    let status = |r: &str| r.split(' ').nth(1).unwrap_or_default().to_owned();

    // No token, a wrong token, and a web page's Origin are all refused.
    assert_eq!(status(&http_post(&addr, "/mcp", &[], &init, "")), "401");
    let wrong = format!("Bearer {}", "0".repeat(64));
    assert_eq!(
        status(&http_post(
            &addr,
            "/mcp",
            &[("Authorization", &wrong)],
            &init,
            ""
        )),
        "401"
    );
    let from_page = http_post(
        &addr,
        "/mcp",
        &[
            ("Authorization", &bearer),
            ("Origin", "https://evil.example"),
        ],
        &init,
        "",
    );
    assert_eq!(status(&from_page), "403", "{from_page}");

    // With the token: handshake, then a tool call answered from serve's
    // latest read.
    let r = http_post(
        &addr,
        "/mcp",
        &[("Authorization", &bearer)],
        &init,
        "serverInfo",
    );
    assert_eq!(status(&r), "200", "{r}");
    assert!(r.contains("\"name\":\"surtr\""), "{r}");
    let session = r
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_owned())
        })
        .expect("session id");
    let headers = [
        ("Authorization", bearer.as_str()),
        ("Mcp-Session-Id", session.as_str()),
        ("MCP-Protocol-Version", "2025-06-18"),
    ];
    let notified = http_post(
        &addr,
        "/mcp",
        &headers,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "",
    );
    assert_eq!(status(&notified), "202", "{notified}");
    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_sensors","arguments":{"kind":"temperature","device":"cpu0"}}}"#;
    let r = http_post(&addr, "/mcp", &headers, call, "cpu0/package/temp");
    assert!(r.contains("cpu0/package/temp"), "{r}");

    assert!(child.wait().unwrap().success());
    let audit = std::fs::read_to_string(dir.join("mcp-audit.jsonl")).unwrap();
    assert!(audit.contains("surtr-http-test 0"), "{audit}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A GET with optional extra headers; returns the raw response.
fn http_get_with(addr: &str, path: &str, headers: &[(&str, &str)]) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

/// Opens a WebSocket at `path` and returns the first text message.
fn websocket_first_text(addr: &str, path: &str, bearer: &str) -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {bearer}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    assert!(status.contains(" 101 "), "{status}");
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
    }
    // One unmasked server frame: FIN+text, then a 7-, 16- or 64-bit length.
    let mut head = [0u8; 2];
    reader.read_exact(&mut head).unwrap();
    assert_eq!(head[0], 0x81, "a final text frame");
    let len = match head[1] & 0x7f {
        126 => {
            let mut n = [0u8; 2];
            reader.read_exact(&mut n).unwrap();
            u64::from(u16::from_be_bytes(n))
        }
        127 => {
            let mut n = [0u8; 8];
            reader.read_exact(&mut n).unwrap();
            u64::from_be_bytes(n)
        }
        n => u64::from(n),
    };
    let mut payload = vec![0u8; usize::try_from(len).unwrap()];
    reader.read_exact(&mut payload).unwrap();
    String::from_utf8(payload).unwrap()
}

#[test]
fn serve_answers_the_rest_api() {
    use std::io::{BufRead, BufReader};
    let dir = std::env::temp_dir().join(format!("surtr-cli-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("history.db");
    let db = db.to_str().unwrap();

    let (code, _) = surtr_code(&[
        "--mock",
        "serve",
        "--api",
        "0.0.0.0:0",
        "--count",
        "1",
        "--db",
        db,
    ]);
    assert_eq!(code, Some(1), "only loopback addresses");

    let mut child = surtr_command()
        .args(["--mock", "serve", "--api", "127.0.0.1:0", "--count", "40"])
        .args(["--interval", "100", "--retain", "0", "--db", db])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    let mut line = String::new();
    while stderr.read_line(&mut line).unwrap() > 0 {
        if let Some(rest) = line.trim().strip_prefix("Serving the API on http://") {
            addr = rest.split_once("/api").map(|(a, _)| a.to_owned());
            break;
        }
        line.clear();
    }
    let addr = addr.expect("serve printed its API address");
    let (ok, token) = surtr(&["token", "--db", db]);
    assert!(ok);
    let bearer = format!("Bearer {}", token.trim());
    let auth = [("Authorization", bearer.as_str())];
    let status = |r: &str| r.split(' ').nth(1).unwrap_or_default().to_owned();
    let body = |r: &str| -> Value {
        serde_json::from_str(r.split_once("\r\n\r\n").map_or("", |(_, b)| b)).unwrap()
    };

    assert_eq!(status(&http_get_with(&addr, "/api/v1/system", &[])), "401");
    let page = http_get_with(
        &addr,
        "/api/v1/system",
        &[
            ("Authorization", &bearer),
            ("Origin", "https://evil.example"),
        ],
    );
    assert_eq!(status(&page), "403");

    let r = http_get_with(&addr, "/api/v1/system", &auth);
    assert_eq!(status(&r), "200", "{r}");
    assert_eq!(body(&r)["providers"][0], "mock");

    let r = http_get_with(&addr, "/api/v1/sensors?kind=temperature&device=cpu0", &auth);
    let sensors = body(&r);
    let sensors = sensors["sensors"].as_array().unwrap();
    assert!(!sensors.is_empty());
    assert!(sensors.iter().all(|s| s["kind"] == "temperature"));

    for path in [
        "/api/v1/capabilities",
        "/api/v1/gpu/processes",
        "/api/v1/gpu/throttle",
    ] {
        assert_eq!(status(&http_get_with(&addr, path, &auth)), "200", "{path}");
    }
    let r = http_get_with(&addr, "/api/v1/history?sensor=no/such", &auth);
    assert_eq!(status(&r), "404");
    assert!(body(&r)["error"].is_string());

    let live = websocket_first_text(&addr, "/api/v1/live?device=gpu0", &bearer);
    let live: Value = serde_json::from_str(&live).unwrap();
    assert!(
        live["sensors"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["device"] == "gpu0")
    );

    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn setup_with_nothing_to_fix() {
    // The mock knows no fixes, so there is nothing to show or run, with or
    // without --dry-run, and never a prompt.
    for args in [&["--mock", "setup"][..], &["--mock", "setup", "--dry-run"]] {
        let (ok, out) = surtr(args);
        assert!(ok);
        assert!(out.starts_with("Nothing to set up"), "{out}");
    }
}

/// The agent skill must stay true to the CLI: valid frontmatter, and every
/// concrete `surtr ... --json` command it names runs and prints JSON.
#[test]
fn agent_skill_matches_the_cli() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/surtr/SKILL.md");
    let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let front = text
        .strip_prefix("---\n")
        .and_then(|t| t.split_once("\n---\n"))
        .map(|(f, _)| f)
        .expect("frontmatter");
    assert!(front.lines().any(|l| l == "name: surtr"));
    let description = front
        .lines()
        .find_map(|l| l.strip_prefix("description: "))
        .expect("description");
    assert!((50..=1024).contains(&description.len()));

    // History and alerts read the data directory; keep the user's out of it.
    let data = std::env::temp_dir().join(format!("surtr-cli-skill-{}", std::process::id()));
    let mut checked = 0;
    for command in text.split('`').skip(1).step_by(2) {
        let Some(args) = command.strip_prefix("surtr ") else {
            continue;
        };
        // Placeholders and commands that write or change things are skipped.
        if !args.contains("--json") || args.contains('<') || args.contains('[') {
            continue;
        }
        let mut argv = vec!["--mock"];
        argv.extend(args.split_whitespace());
        if argv[1] == "status" || argv[1] == "sensors" {
            argv.extend(["--interval", "0"]);
        }
        if argv[1] == "watch" {
            argv.extend(["--interval", "0"]);
        }
        let out = surtr_command()
            .env("SURTR_DATA_DIR", &data)
            .args(&argv)
            .output()
            .unwrap();
        // status exits 2 on the mock machine (warning); anything but an error
        // running surtr is fine.
        assert_ne!(out.status.code(), Some(1), "{args}: {out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        let first = stdout.lines().next().unwrap_or_default();
        assert!(
            serde_json::from_str::<Value>(&stdout).is_ok()
                || serde_json::from_str::<Value>(first).is_ok(),
            "{args}: {stdout}"
        );
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} commands checked");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn stress_cpu_passes_on_the_mock() {
    let (code, out) = surtr_code(&[
        "--mock",
        "--json",
        "stress",
        "cpu",
        "--duration",
        "2s",
        "--threads",
        "2",
    ]);
    assert_eq!(code, Some(0), "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcome"]["result"], "passed");
    assert_eq!(v["threads"], 2);
    assert!(
        v["iterations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.as_u64() > Some(0))
    );
    assert!(v["failures"].as_array().unwrap().is_empty());
    assert!(v["watched"][0]["stop_at"].as_f64().is_some());
}

#[test]
fn stress_stops_at_the_temperature_limit() {
    // The mock CPU runs at about 56 °C; a 40 °C limit stops the run at once.
    let dir = std::env::temp_dir().join(format!("surtr-cli-stress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "[stress]\nmax_temp_c = 40\n").unwrap();
    let out = surtr_command()
        .env("SURTR_CONFIG_DIR", &dir)
        .args([
            "--mock",
            "--json",
            "stress",
            "cpu",
            "--duration",
            "20s",
            "--threads",
            "1",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"]["result"], "temperature_limit");
    assert_eq!(v["outcome"]["limit"], 40.0);
    assert!(v["duration_ms"].as_u64().unwrap() < 10_000, "stopped early");

    // The settings cap the duration too.
    std::fs::write(dir.join("config.toml"), "[stress]\nmax_duration = \"1m\"\n").unwrap();
    let out = surtr_command()
        .env("SURTR_CONFIG_DIR", &dir)
        .args(["--mock", "stress", "cpu", "--duration", "2m"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("longer than the 1m 00s limit"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh data directory for one test.
fn data_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("surtr-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn stress_runs_are_recorded_and_reported() {
    let data = data_dir("runs");
    let run = |args: &[&str]| {
        surtr_command()
            .env("SURTR_DATA_DIR", &data)
            .args(args)
            .output()
            .unwrap()
    };
    let out = run(&[
        "--mock",
        "stress",
        "cpu",
        "--duration",
        "2s",
        "--threads",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Saved as run "));

    let out = run(&["--json", "runs"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let runs = v["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["state"], "done");
    assert_eq!(runs[0]["report"]["outcome"]["result"], "passed");
    assert!(!runs[0]["timeline"].as_array().unwrap().is_empty());
    assert!(runs[0]["id"].as_str().unwrap().ends_with("-cpu"));

    let text = String::from_utf8(run(&["runs"]).stdout).unwrap();
    assert!(text.contains("passed"), "{text}");
    let text = String::from_utf8(run(&["runs", "show", "latest"]).stdout).unwrap();
    assert!(text.contains("Passed: no errors"), "{text}");

    let page = data.join("report.html");
    let out = run(&["runs", "report", "latest", "--out", page.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.contains("Passed: no errors"));
    assert_eq!(run(&["runs", "show", "nope"]).status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_run_cut_short_is_recovered_as_interrupted() {
    let data = data_dir("journal");
    let mut host = surtr_command()
        .env("SURTR_DATA_DIR", &data)
        .args([
            "--mock",
            "stress",
            "cpu",
            "--duration",
            "30s",
            "--threads",
            "1",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Wait until the journal holds a few seconds of timeline.
    let runs = data.join("runs");
    let mut ticks = 0;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let journal = std::fs::read_dir(&runs)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "json"));
        if let Some(text) = journal.and_then(|p| std::fs::read_to_string(p).ok())
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            ticks = v["timeline"].as_array().map_or(0, Vec::len);
            if ticks >= 2 {
                break;
            }
        }
    }
    assert!(ticks >= 2, "the journal is written during the run");
    // As if the machine reset: the host dies without finishing the record.
    host.kill().unwrap();
    host.wait().unwrap();

    let out = surtr_command()
        .env("SURTR_DATA_DIR", &data)
        .args(["--json", "runs"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("was interrupted"));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let run = &v["runs"][0];
    assert_eq!(run["state"], "interrupted");
    assert!(run["report"].is_null());
    assert!(run["timeline"].as_array().unwrap().len() >= 2, "{run}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn mcp_stress_requests_need_opt_in_and_a_person() {
    let call = |name: &str, arguments: Value| {
        (
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )
    };
    let tool_names = |list: &Value| -> Vec<String> {
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect()
    };
    let data = data_dir("mcp-stress");
    let config = data.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let db = data.join("history.db");
    let args = ["--mock", "mcp", "--db", db.to_str().unwrap()];
    let envs = [
        ("SURTR_DATA_DIR", data.as_path()),
        ("SURTR_CONFIG_DIR", config.as_path()),
    ];

    // Off by default: the stress tools don't exist; recorded runs can be read.
    let r = mcp_session_with(&envs, &args, &[("tools/list", serde_json::json!({}))]);
    let names = tool_names(&r[0]);
    assert!(!names.iter().any(|n| n == "start_stress_test"), "{names:?}");
    assert!(names.iter().any(|n| n == "list_runs"));

    std::fs::write(config.join("config.toml"), "[mcp]\nallow_stress = true\n").unwrap();
    let r = mcp_session_with(
        &envs,
        &args,
        &[
            ("tools/list", serde_json::json!({})),
            call("start_stress_test", serde_json::json!({ "minutes": 99 })),
            call(
                "start_stress_test",
                serde_json::json!({ "minutes": 1, "threads": 1 }),
            ),
        ],
    );
    let names = tool_names(&r[0]);
    for tool in ["start_stress_test", "get_run_status", "stop_stress_test"] {
        assert!(names.iter().any(|n| n == tool), "{tool} missing");
    }
    // Starting is not read-only; the annotations say so.
    let start = r[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "start_stress_test")
        .unwrap();
    assert_eq!(start["annotations"]["readOnlyHint"], false);
    assert_eq!(r[1]["isError"], true, "longer than the owner's limit");
    let asked = tool_text(&r[2]);
    assert_eq!(asked["status"], "awaiting_confirmation");
    let code = asked["request"].as_str().unwrap().to_owned();
    assert!(
        asked["next"]
            .as_str()
            .unwrap()
            .contains(&format!("surtr runs confirm {code}"))
    );

    // Nothing runs until a person confirms, and that needs a terminal.
    let confirm = surtr_command()
        .envs(envs)
        .args(["--mock", "runs", "confirm", &code])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(confirm.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&confirm.stderr).contains("interactive terminal"));

    let status = |code: &str| {
        let r = mcp_session_with(
            &envs,
            &args,
            &[call("get_run_status", serde_json::json!({ "id": code }))],
        );
        tool_text(&r[0])
    };
    assert_eq!(status(&code)["status"], "pending");

    // The person says no.
    let deny = surtr_command()
        .envs(envs)
        .args(["runs", "deny", &code])
        .output()
        .unwrap();
    assert_eq!(deny.status.code(), Some(0));
    assert_eq!(status(&code)["status"], "denied");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn mcp_can_stop_a_running_stress_test() {
    let data = data_dir("mcp-stop");
    let db = data.join("history.db");
    // The stop tool comes with the stress opt-in.
    let config = data.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[mcp]
allow_stress = true
",
    )
    .unwrap();
    let host = surtr_command()
        .env("SURTR_DATA_DIR", &data)
        .args([
            "--mock",
            "--json",
            "stress",
            "cpu",
            "--duration",
            "30s",
            "--threads",
            "1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Wait until the run is recorded as running.
    let args = ["--mock", "mcp", "--db", db.to_str().unwrap()];
    let envs = [
        ("SURTR_DATA_DIR", data.as_path()),
        ("SURTR_CONFIG_DIR", config.as_path()),
    ];
    let call = |name: &str| {
        (
            "tools/call",
            serde_json::json!({ "name": name, "arguments": { "id": "latest" } }),
        )
    };
    let mut stopped = Value::Null;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let r = mcp_session_with(&envs, &args, &[call("stop_stress_test")]);
        stopped = tool_text(&r[0]);
        if stopped["status"] == "stop requested" {
            break;
        }
    }
    assert_eq!(stopped["status"], "stop requested", "{stopped}");
    let out = host.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2), "stopped early");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"]["result"], "stopped");
    assert!(v["duration_ms"].as_u64().unwrap() < 20_000);

    let r = mcp_session_with(
        &envs,
        &args,
        &[(
            "tools/call",
            serde_json::json!({ "name": "list_runs", "arguments": {} }),
        )],
    );
    let runs = tool_text(&r[0]);
    assert_eq!(runs["runs"][0]["state"], "done");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn stress_threads_go_where_asked() {
    let data = data_dir("placement");
    let run = |args: &[&str]| {
        surtr_command()
            .env("SURTR_DATA_DIR", &data)
            .args(["--mock", "--json", "stress", "cpu", "--duration", "1s"])
            .args(args)
            .output()
            .unwrap()
    };
    // Hand-picked CPUs, recorded in the run.
    assert_eq!(run(&["--cpus", "0"]).status.code(), Some(0));
    let runs = surtr_command()
        .env("SURTR_DATA_DIR", &data)
        .args(["--json", "runs", "show", "latest"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&runs.stdout).unwrap();
    assert_eq!(v["cpus"], serde_json::json!([0]));
    assert_eq!(v["threads"], 1);

    // More threads than CPUs, a CPU that isn't there, and conflicting flags
    // are refused before anything runs.
    for args in [
        &["--threads", "100000"][..],
        &["--cpus", "100000"],
        &["--cpus", "0", "--threads", "1"],
    ] {
        assert_ne!(run(args).status.code(), Some(0), "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&data);
}
