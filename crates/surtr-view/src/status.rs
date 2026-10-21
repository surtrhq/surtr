//! The health summary behind `surtr status` and the Overview screens: a
//! one-line reading per part and plain-language findings (plan, section 2.2).

use surtr_core::Sample;
use surtr_model::{
    Capabilities, Feature, SensorKind, Support, SystemInfo, ThrottleReason, ThrottleState, Unit,
};

use crate::{Cell, Level, Role, Row, format, health};

/// Something worth telling the user. `level` is `None` for information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// How serious it is.
    pub level: Option<Level>,
    /// One plain-language sentence.
    pub message: String,
}

/// The health summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The worst level of all findings.
    pub level: Level,
    /// One row per part: CPU, memory, each GPU.
    pub parts: Vec<Row>,
    /// Problems first, then information.
    pub findings: Vec<Finding>,
}

fn value(samples: &[Sample], pick: impl Fn(&Sample) -> bool) -> Option<(f64, &Sample)> {
    samples
        .iter()
        .filter(|s| pick(s))
        .find_map(|s| Some((s.value?, s)))
}

/// The hottest temperature among samples matching `pick`.
fn hottest(samples: &[Sample], pick: impl Fn(&Sample) -> bool) -> Option<(f64, &Sample)> {
    samples
        .iter()
        .filter(|s| s.descriptor.kind == SensorKind::Temperature && pick(s))
        .filter_map(|s| Some((s.value?, s)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
}

/// A whole-CPU sensor of `kind` (not a per-thread one), such as total load.
fn whole_cpu(s: &Sample, kind: SensorKind) -> bool {
    s.descriptor.device.starts_with("cpu")
        && s.descriptor.kind == kind
        && !s.descriptor.id.0.contains("thread")
}

fn joined(parts: Vec<String>) -> Cell {
    if parts.is_empty() {
        Cell::new(format::MISSING, Role::Missing)
    } else {
        Cell::new(parts.join(" · "), Role::Text)
    }
}

/// Builds the summary from one snapshot of everything.
pub fn status(
    system: &SystemInfo,
    samples: &[Sample],
    throttle: &[ThrottleState],
    capabilities: &Capabilities,
) -> Status {
    let mut parts = Vec::new();
    let mut findings = Vec::new();

    // CPU: load, clock, hottest temperature.
    let is_cpu = |s: &Sample| s.descriptor.device.starts_with("cpu");
    let mut cpu = Vec::new();
    if let Some((v, _)) = value(samples, |s| whole_cpu(s, SensorKind::Load)) {
        cpu.push(format!("{} load", format::value(Unit::Percent, v)));
    }
    if let Some((v, _)) = value(samples, |s| whole_cpu(s, SensorKind::Clock)) {
        cpu.push(format::value(Unit::Megahertz, v));
    }
    if let Some((v, _)) = hottest(samples, is_cpu) {
        cpu.push(format::value(Unit::Celsius, v));
    }
    parts.push(Row {
        label: "CPU".to_owned(),
        cells: vec![joined(cpu)],
    });

    // Memory: used of total.
    let total = system.memory.total_bytes.as_ref().map(|f| f.value);
    let used = value(samples, |s| s.descriptor.id.0 == "memory/used").map(|(v, _)| v);
    let memory = match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            let percent = used / total as f64 * 100.0;
            if percent >= 90.0 {
                findings.push(Finding {
                    level: Some(Level::Warn),
                    message: format!(
                        "Memory is almost full ({percent:.0} % used). Close programs or add memory."
                    ),
                });
            }
            vec![format!(
                "{} of {} ({percent:.0} %)",
                format::value(Unit::Byte, used),
                format::bytes(total)
            )]
        }
        (None, Some(total)) => vec![format!("{} total", format::bytes(total))],
        _ => Vec::new(),
    };
    parts.push(Row {
        label: "Memory".to_owned(),
        cells: vec![joined(memory)],
    });

    // GPUs, in sensor order.
    let mut gpus: Vec<&str> = Vec::new();
    for s in samples {
        let d = s.descriptor.device.as_str();
        if !is_cpu(s) && d != "memory" && !d.starts_with("hwmon") && !gpus.contains(&d) {
            let is_gpu = samples
                .iter()
                .any(|x| x.descriptor.device == d && x.descriptor.label.contains("GPU"));
            if is_gpu {
                gpus.push(d);
            }
        }
    }
    for device in gpus {
        let mine = |s: &Sample| s.descriptor.device == device;
        let mut gpu = Vec::new();
        if let Some((v, _)) = hottest(samples, mine) {
            gpu.push(format::value(Unit::Celsius, v));
        }
        if let Some((v, _)) = value(samples, |s| mine(s) && s.descriptor.label == "Board power") {
            gpu.push(format::value(Unit::Watt, v));
        }
        if let Some((v, _)) = value(samples, |s| mine(s) && s.descriptor.label == "GPU load") {
            gpu.push(format!("{} load", format::value(Unit::Percent, v)));
        }
        if let Some((v, _)) = value(samples, |s| {
            mine(s) && s.descriptor.kind == SensorKind::Memory
        }) {
            gpu.push(format!("{} VRAM", format::value(Unit::Byte, v)));
        }
        parts.push(Row {
            label: device.to_owned(),
            cells: vec![joined(gpu)],
        });
    }

    // Temperatures above their thresholds.
    for s in samples {
        let Some(v) = s.value else { continue };
        let Some(level) = health::level(&s.descriptor, v).filter(|&l| l > Level::Ok) else {
            continue;
        };
        let advice = match level {
            Level::Critical => "Reduce the load and check cooling now.",
            _ => "Check cooling and airflow.",
        };
        let word = if level == Level::Critical {
            "critical"
        } else {
            "high"
        };
        findings.push(Finding {
            level: Some(level),
            message: format!(
                "{}: {} is {word} ({}). {advice}",
                s.descriptor.device,
                s.descriptor.label,
                format::value(Unit::Celsius, v)
            ),
        });
    }

    // Throttling that costs performance (idle and display clocks don't).
    for t in throttle {
        let serious: Vec<ThrottleReason> = t
            .reasons
            .iter()
            .copied()
            .filter(|r| !matches!(r, ThrottleReason::Idle | ThrottleReason::DisplayClocks))
            .collect();
        if serious.is_empty() {
            continue;
        }
        let level = serious
            .iter()
            .map(|&r| match format::throttle_role(r) {
                Role::Critical => Level::Critical,
                _ => Level::Warn,
            })
            .max()
            .unwrap_or(Level::Warn);
        let reasons: Vec<&str> = serious.iter().map(|&r| format::throttle(r)).collect();
        findings.push(Finding {
            level: Some(level),
            message: format!("{} is slowed down by: {}.", t.device, reasons.join(", ")),
        });
    }

    // What Surtr can't see, so "no problems" isn't over-trusted.
    if !matches!(
        capabilities.support(Feature::CpuTemperature),
        Some(Support::Available)
    ) {
        let why = match capabilities.support(Feature::CpuTemperature) {
            Some(Support::NeedsPrivilegedBackend) => " without the privileged backend",
            Some(Support::NeedsElevation) => " without admin or root rights",
            _ => "",
        };
        findings.push(Finding {
            level: None,
            message: format!("CPU temperature isn't readable on this machine{why}, so overheating can't be detected."),
        });
    }

    findings.sort_by_key(|f| std::cmp::Reverse(f.level));
    let level = findings
        .iter()
        .filter_map(|f| f.level)
        .max()
        .unwrap_or(Level::Ok);
    Status {
        level,
        parts,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{Provider, mock::MockProvider};

    fn snapshot(
        mock: &MockProvider,
    ) -> (SystemInfo, Vec<Sample>, Vec<ThrottleState>, Capabilities) {
        let readings = mock.read_sensors().unwrap();
        let samples = mock
            .sensors()
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
            .collect();
        (
            mock.system_info().unwrap(),
            samples,
            mock.throttle_reasons().unwrap(),
            mock.capabilities(),
        )
    }

    #[test]
    fn mock_desktop_summary() {
        let (system, samples, throttle, caps) = snapshot(&MockProvider::desktop());
        let s = status(&system, &samples, &throttle, &caps);
        let labels: Vec<_> = s.parts.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["CPU", "Memory", "gpu0"]);
        assert_eq!(s.parts[0].cells[0].text, "40.0 % load · 4200 MHz · 55.0 °C");
        // The mock GPU is held at its power limit.
        assert_eq!(s.level, Level::Warn);
        assert_eq!(
            s.findings[0].message,
            "gpu0 is slowed down by: power limit."
        );
    }

    #[test]
    fn hot_cpu_is_critical() {
        let (system, mut samples, _, caps) = snapshot(&MockProvider::desktop());
        samples[0].value = Some(97.0);
        let s = status(&system, &samples, &[], &caps);
        assert_eq!(s.level, Level::Critical);
        assert!(
            s.findings[0]
                .message
                .contains("Package temperature is critical (97.0 °C)")
        );
    }

    #[test]
    fn unreadable_cpu_temperature_is_noted() {
        let (system, samples, _, mut caps) = snapshot(&MockProvider::desktop());
        caps.set(Feature::CpuTemperature, Support::NeedsPrivilegedBackend);
        let s = status(&system, &samples, &[], &caps);
        assert_eq!(s.level, Level::Ok);
        assert!(s.findings[0].level.is_none());
        assert!(
            s.findings[0]
                .message
                .contains("without the privileged backend")
        );
    }
}
