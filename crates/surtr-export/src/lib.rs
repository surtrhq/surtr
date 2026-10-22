//! Exporters (plan, section 5.5). For now: the Prometheus text exposition
//! format behind `surtr serve --metrics`.
//!
//! Metric names follow Prometheus conventions: base units in the name
//! (`_celsius`, `_watts`, `_hertz`, `_bytes`), ratios from 0 to 1 instead of
//! percentages. Labels are low-cardinality and carry no host name (plan,
//! section 6.1); the scraper adds its own `instance` label.

#![forbid(unsafe_code)]

use std::{collections::BTreeMap, fmt::Write};

use surtr_core::Sample;
use surtr_model::{SensorKind, ThrottleReason, ThrottleState, Unit};

/// Every metric name Surtr can export. Dashboards may only use these (a test
/// checks `dashboards/*.json`).
pub const METRICS: [&str; 13] = [
    "surtr_info",
    "surtr_health_level",
    "surtr_temperature_celsius",
    "surtr_power_watts",
    "surtr_voltage_volts",
    "surtr_current_amperes",
    "surtr_clock_hertz",
    "surtr_load_ratio",
    "surtr_fan_rpm",
    "surtr_fan_ratio",
    "surtr_memory_used_bytes",
    "surtr_energy_joules",
    "surtr_gpu_throttle_active",
];

/// How one sensor kind and unit is exported: metric name, help text and the
/// factor from Surtr's unit to the metric's base unit.
fn family(kind: SensorKind, unit: Unit) -> Option<(&'static str, &'static str, f64)> {
    Some(match (kind, unit) {
        (SensorKind::Temperature, Unit::Celsius) => (
            "surtr_temperature_celsius",
            "Temperature reported by a sensor.",
            1.0,
        ),
        (SensorKind::Power, Unit::Watt) => ("surtr_power_watts", "Power draw or limit.", 1.0),
        (SensorKind::Voltage, Unit::Volt) => ("surtr_voltage_volts", "Voltage.", 1.0),
        (SensorKind::Current, Unit::Ampere) => ("surtr_current_amperes", "Current.", 1.0),
        (SensorKind::Clock, Unit::Megahertz) => ("surtr_clock_hertz", "Clock frequency.", 1e6),
        (SensorKind::Load, Unit::Percent) => ("surtr_load_ratio", "Utilization from 0 to 1.", 0.01),
        (SensorKind::Fan, Unit::Rpm) => {
            ("surtr_fan_rpm", "Fan speed in revolutions per minute.", 1.0)
        }
        (SensorKind::Fan, Unit::Percent) => (
            "surtr_fan_ratio",
            "Fan speed as a fraction of its maximum.",
            0.01,
        ),
        (SensorKind::Memory, Unit::Byte) => ("surtr_memory_used_bytes", "Memory in use.", 1.0),
        (SensorKind::Energy, Unit::Joule) => ("surtr_energy_joules", "Accumulated energy.", 1.0),
        _ => return None,
    })
}

/// A label value escaped for the text format.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Serde's name for an enum variant, for example `power_cap`.
fn name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

fn header(out: &mut String, metric: &str, help: &str) {
    // Writing to a String can't fail.
    let _ = writeln!(out, "# HELP {metric} {help}");
    let _ = writeln!(out, "# TYPE {metric} gauge");
}

/// The Prometheus text exposition of one snapshot. `health` is 0 ok,
/// 1 warning, 2 critical, when known.
pub fn prometheus(samples: &[Sample], throttle: &[ThrottleState], health: Option<u8>) -> String {
    let mut families: BTreeMap<&'static str, (&'static str, Vec<String>)> = BTreeMap::new();
    for s in samples {
        let d = &s.descriptor;
        let (Some(value), Some((metric, help, factor))) = (s.value, family(d.kind, d.unit)) else {
            continue;
        };
        let line = format!(
            "{metric}{{provider=\"{}\",device=\"{}\",sensor=\"{}\",label=\"{}\"}} {}",
            escape(&d.source.provider),
            escape(&d.device),
            escape(&d.id.0),
            escape(&d.label),
            value * factor
        );
        families
            .entry(metric)
            .or_insert((help, Vec::new()))
            .1
            .push(line);
    }

    let mut out = String::new();
    header(&mut out, "surtr_info", "Surtr version.");
    let _ = writeln!(
        out,
        "surtr_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(level) = health {
        header(
            &mut out,
            "surtr_health_level",
            "Overall health: 0 ok, 1 warning, 2 critical.",
        );
        let _ = writeln!(out, "surtr_health_level {level}");
    }
    for (metric, (help, lines)) in &families {
        header(&mut out, metric, help);
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !throttle.is_empty() {
        header(
            &mut out,
            "surtr_gpu_throttle_active",
            "1 when the reason currently limits the GPU's clock.",
        );
        for t in throttle {
            for reason in ThrottleReason::ALL {
                let active = u8::from(t.reasons.contains(&reason));
                let _ = writeln!(
                    out,
                    "surtr_gpu_throttle_active{{device=\"{}\",reason=\"{}\"}} {active}",
                    escape(&t.device),
                    name(&reason)
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{Provider, mock::MockProvider};

    fn samples(mock: &MockProvider) -> Vec<Sample> {
        let readings = mock.read_sensors().unwrap();
        mock.sensors()
            .unwrap()
            .into_iter()
            .map(|descriptor| {
                let r = readings.iter().find(|r| r.id == descriptor.id);
                Sample {
                    value: r.map(|r| r.value),
                    at: r.map(|r| r.at),
                    descriptor,
                }
            })
            .collect()
    }

    #[test]
    fn mock_exposition() {
        let mock = MockProvider::desktop();
        let text = prometheus(&samples(&mock), &mock.throttle_reasons().unwrap(), Some(1));
        for expected in [
            "# TYPE surtr_temperature_celsius gauge\n",
            "surtr_temperature_celsius{provider=\"mock\",device=\"cpu0\",sensor=\"cpu0/package/temp\",label=\"Package temperature\"} 55\n",
            "surtr_load_ratio{provider=\"mock\",device=\"cpu0\",sensor=\"cpu0/load\",label=\"Total load\"} 0.4\n",
            "surtr_clock_hertz{provider=\"mock\",device=\"cpu0\",sensor=\"cpu0/clock\",label=\"Effective clock\"} 4200000000\n",
            "surtr_health_level 1\n",
            "surtr_gpu_throttle_active{device=\"gpu0\",reason=\"power_cap\"} 1\n",
            "surtr_gpu_throttle_active{device=\"gpu0\",reason=\"idle\"} 0\n",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        // Each family's HELP and TYPE appear once.
        assert_eq!(text.matches("# TYPE surtr_temperature_celsius").count(), 1);
    }

    fn metric_names(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut rest = text;
        while let Some(i) = rest.find("surtr_") {
            let tail = &rest[i..];
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            names.push(tail[..end].to_owned());
            rest = &tail[end..];
        }
        names
    }

    // Everything the exporter emits is in METRICS.
    #[test]
    fn exported_names_are_listed() {
        let mock = MockProvider::desktop();
        let text = prometheus(&samples(&mock), &mock.throttle_reasons().unwrap(), Some(0));
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let name = line.split(['{', ' ']).next().unwrap();
            assert!(METRICS.contains(&name), "{name} is not in METRICS");
        }
    }

    // Every query in the shipped dashboards uses a metric Surtr exports.
    #[test]
    fn dashboards_use_exported_metrics() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dashboards");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let dashboard: serde_json::Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                dashboard["panels"]
                    .as_array()
                    .is_some_and(|p| !p.is_empty())
            );
            for name in metric_names(&text) {
                assert!(
                    METRICS.contains(&name.as_str()),
                    "{}: unknown metric {name}",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(
            checked > 10,
            "no dashboard queries found in {}",
            dir.display()
        );
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn unread_sensors_are_left_out() {
        let mock = MockProvider::desktop();
        let mut s = samples(&mock);
        for x in &mut s {
            x.value = None;
        }
        let text = prometheus(&s, &[], None);
        assert!(!text.contains("surtr_temperature_celsius{"));
        assert!(!text.contains("surtr_health_level"));
        assert!(text.contains("surtr_info{version="));
    }
}
