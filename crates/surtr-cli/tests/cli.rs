//! Runs the real `surtr` binary.

use std::process::Command;

use serde_json::Value;

fn surtr(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_surtr"))
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
    assert_eq!(v["provider"], "mock");
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
