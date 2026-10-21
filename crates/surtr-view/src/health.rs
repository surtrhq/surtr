//! Health levels for sensor values.
//!
//! Until vendor-reported limits (Tjmax, GPU slowdown temperatures) and the SKU
//! database exist, temperatures use conservative generic thresholds (plan,
//! section 2.2). Other kinds have no level yet.

use surtr_model::{SensorDescriptor, SensorKind};

use crate::Role;

/// How healthy a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Normal.
    Ok,
    /// Worth a look.
    Warn,
    /// Act now.
    Critical,
}

impl Level {
    /// The theme role for this level.
    pub fn role(self) -> Role {
        match self {
            Self::Ok => Role::Ok,
            Self::Warn => Role::Warn,
            Self::Critical => Role::Critical,
        }
    }

    /// A one-word label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warm",
            Self::Critical => "hot",
        }
    }
}

fn is_gpu(device: &str) -> bool {
    ["gpu", "nvidia", "amd"]
        .iter()
        .any(|p| device.starts_with(p))
}

/// `(warn, critical)` temperature thresholds in °C for a sensor.
pub fn temperature_limits(sensor: &SensorDescriptor) -> (f64, f64) {
    let label = sensor.label.to_ascii_lowercase();
    if sensor.device.starts_with("cpu") {
        (85.0, 95.0)
    } else if is_gpu(&sensor.device) && (label.contains("hotspot") || label.contains("memory")) {
        (95.0, 105.0)
    } else if is_gpu(&sensor.device) {
        (83.0, 90.0)
    } else if sensor.device.starts_with("nvme") {
        (70.0, 80.0)
    } else {
        (80.0, 95.0)
    }
}

/// The level of `value` for `sensor`, or `None` when Surtr doesn't judge
/// that kind of sensor.
pub fn level(sensor: &SensorDescriptor, value: f64) -> Option<Level> {
    match sensor.kind {
        SensorKind::Temperature => {
            let (warn, critical) = temperature_limits(sensor);
            Some(if value >= critical {
                Level::Critical
            } else if value >= warn {
                Level::Warn
            } else {
                Level::Ok
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_model::{SensorId, Source, Unit};

    fn sensor(device: &str, kind: SensorKind, label: &str) -> SensorDescriptor {
        SensorDescriptor {
            id: SensorId::new("x"),
            kind,
            unit: kind.unit(),
            device: device.to_owned(),
            label: label.to_owned(),
            source: Source::new("test", "test"),
        }
    }

    #[test]
    fn cpu_and_gpu_temperatures() {
        let cpu = sensor("cpu0", SensorKind::Temperature, "Package temperature");
        assert_eq!(level(&cpu, 60.0), Some(Level::Ok));
        assert_eq!(level(&cpu, 88.0), Some(Level::Warn));
        assert_eq!(level(&cpu, 96.0), Some(Level::Critical));
        let gpu = sensor("nvidia0", SensorKind::Temperature, "GPU temperature");
        assert_eq!(level(&gpu, 84.0), Some(Level::Warn));
        let hotspot = sensor("amd0", SensorKind::Temperature, "Temperature (hotspot)");
        assert_eq!(level(&hotspot, 84.0), Some(Level::Ok));
    }

    #[test]
    fn other_kinds_are_not_judged() {
        let load = sensor("cpu", SensorKind::Load, "Total load");
        assert_eq!(level(&load, 100.0), None);
        assert_eq!(load.unit, Unit::Percent);
    }
}
