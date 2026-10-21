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

// On Windows and Linux the total CPU load must be readable without a driver.
// The OS may mark a single sample invalid (Surtr then leaves it out), so a
// few attempts are allowed; a load that never appears still fails.
#[test]
fn native_sensors_json() {
    if !cfg!(any(windows, target_os = "linux")) {
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
