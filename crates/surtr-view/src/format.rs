//! Text for values and names, shared by every front end.

use surtr_model::{
    Arch, CpuVendor, Feature, GpuProcessKind, GpuVendor, Remedy, Support, ThrottleReason,
    Timestamp, Unit,
};

use crate::Role;

/// Shown for a value Surtr doesn't have.
pub const MISSING: &str = "not available";

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// A value in its unit, for example `55.0 °C` or `4.0 GiB`.
pub fn value(unit: Unit, v: f64) -> String {
    match unit {
        Unit::Celsius => format!("{v:.1} °C"),
        Unit::Watt => format!("{v:.1} W"),
        Unit::Volt => format!("{v:.3} V"),
        Unit::Ampere => format!("{v:.2} A"),
        Unit::Megahertz => format!("{v:.0} MHz"),
        Unit::Percent => format!("{v:.1} %"),
        Unit::Rpm => format!("{v:.0} RPM"),
        Unit::Byte => format!("{:.1} GiB", v / GIB),
        Unit::Joule => format!("{v:.1} J"),
        _ => format!("{v}"),
    }
}

/// Bytes as GiB.
pub fn bytes(b: u64) -> String {
    value(Unit::Byte, b as f64)
}

/// Architecture name.
pub fn arch(arch: &Arch) -> String {
    match arch {
        Arch::X86_64 => "x86-64".to_owned(),
        Arch::Aarch64 => "ARM64".to_owned(),
        Arch::Other(name) => name.clone(),
    }
}

/// CPU vendor name.
pub fn cpu_vendor(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Intel => "Intel".to_owned(),
        CpuVendor::Amd => "AMD".to_owned(),
        CpuVendor::Arm => "Arm".to_owned(),
        CpuVendor::Apple => "Apple".to_owned(),
        CpuVendor::Qualcomm => "Qualcomm".to_owned(),
        CpuVendor::Other(name) => name.clone(),
    }
}

/// GPU vendor name.
pub fn gpu_vendor(vendor: &GpuVendor) -> String {
    match vendor {
        GpuVendor::Nvidia => "NVIDIA".to_owned(),
        GpuVendor::Amd => "AMD".to_owned(),
        GpuVendor::Intel => "Intel".to_owned(),
        GpuVendor::Apple => "Apple".to_owned(),
        GpuVendor::Other(name) => name.clone(),
    }
}

/// Feature name.
pub fn feature(feature: Feature) -> &'static str {
    match feature {
        Feature::CpuIdentity => "CPU identity",
        Feature::CpuLoad => "CPU load",
        Feature::CpuClock => "CPU clocks",
        Feature::CpuTemperature => "CPU temperature",
        Feature::CpuPower => "CPU power",
        Feature::CpuVoltage => "CPU voltage",
        Feature::MemoryInfo => "Memory size",
        Feature::MemoryTimings => "Memory timings",
        Feature::GpuIdentity => "GPU identity",
        Feature::GpuSensors => "GPU sensors",
        Feature::GpuProcesses => "GPU processes",
        Feature::GpuThrottleReasons => "GPU throttle reasons",
        Feature::StorageHealth => "Storage health",
        Feature::BoardSensors => "Board sensors",
        Feature::FanControl => "Fan control",
        Feature::CpuTuning => "CPU tuning",
        Feature::GpuTuning => "GPU tuning",
        _ => "Other",
    }
}

/// Support status text and its role; `None` means no provider mentioned it.
pub fn support(support: Option<&Support>) -> (String, Role) {
    match support {
        Some(Support::Available) => ("available".to_owned(), Role::Ok),
        Some(Support::NeedsElevation) => ("needs admin/root".to_owned(), Role::Warn),
        Some(Support::NeedsPrivilegedBackend) => {
            ("needs privileged backend".to_owned(), Role::Warn)
        }
        Some(Support::NoHardware) => ("no hardware".to_owned(), Role::Muted),
        Some(Support::Unsupported { reason }) => (format!("unsupported ({reason})"), Role::Missing),
        None => ("unknown".to_owned(), Role::Missing),
    }
}

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The last `width` values as a block-character sparkline, scaled to `range`
/// or to the values' own span.
pub fn spark(
    values: impl IntoIterator<Item = f64>,
    width: usize,
    range: Option<(f64, f64)>,
) -> String {
    let values: Vec<f64> = values.into_iter().collect();
    let values = &values[values.len().saturating_sub(width)..];
    if values.is_empty() {
        return String::new();
    }
    let (lo, hi) = range.unwrap_or_else(|| {
        let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        // Span at least 5 % of the magnitude, so noise (a clock moving from
        // 5500 to 5501 MHz) draws as a steady line instead of a jump.
        let min_span = lo.abs().max(hi.abs()) * 0.05;
        if hi - lo < min_span {
            let mid = (hi + lo) / 2.0;
            (mid - min_span / 2.0, mid + min_span / 2.0)
        } else {
            (lo, hi)
        }
    });
    values
        .iter()
        .map(|v| {
            if hi <= lo {
                return BARS[0];
            }
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            BARS[((t * 7.0).round() as usize).min(7)]
        })
        .collect()
}

/// The fixed range a sparkline of `unit` should use: 0-100 for percentages,
/// otherwise the values' own span.
pub fn spark_range(unit: Unit) -> Option<(f64, f64)> {
    (unit == Unit::Percent).then_some((0.0, 100.0))
}

/// A remedy as one line: what to do, then the command or link.
pub fn remedy(r: &Remedy) -> String {
    match (&r.command, &r.url) {
        (Some(c), _) => format!("{}: {c}", r.text),
        (None, Some(u)) => format!("{}: {u}", r.text),
        (None, None) => r.text.clone(),
    }
}

/// Throttle reason text.
pub fn throttle(reason: ThrottleReason) -> &'static str {
    match reason {
        ThrottleReason::Idle => "idle",
        ThrottleReason::ApplicationClocks => "application clock setting",
        ThrottleReason::PowerCap => "power limit",
        ThrottleReason::HardwareSlowdown => "hardware slowdown",
        ThrottleReason::SyncBoost => "sync boost",
        ThrottleReason::SoftwareThermal => "thermal limit (driver)",
        ThrottleReason::HardwareThermal => "thermal limit (hardware)",
        ThrottleReason::HardwarePowerBrake => "power brake",
        ThrottleReason::DisplayClocks => "display clock setting",
        _ => "other",
    }
}

/// How serious a throttle reason is: thermal and hardware limits matter,
/// idle doesn't.
pub fn throttle_role(reason: ThrottleReason) -> Role {
    match reason {
        ThrottleReason::Idle | ThrottleReason::DisplayClocks => Role::Muted,
        ThrottleReason::HardwareSlowdown
        | ThrottleReason::HardwareThermal
        | ThrottleReason::HardwarePowerBrake => Role::Critical,
        _ => Role::Warn,
    }
}

/// GPU process kind text.
pub fn process_kind(kind: GpuProcessKind) -> &'static str {
    match kind {
        GpuProcessKind::Compute => "compute",
        GpuProcessKind::Graphics => "graphics",
        GpuProcessKind::Unknown => "-",
    }
}

/// A timestamp as UTC ISO 8601, for example `2026-10-21T21:50:34Z`.
pub fn iso_utc(t: Timestamp) -> String {
    let secs = t.0 / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_by_unit() {
        assert_eq!(value(Unit::Celsius, 55.0), "55.0 °C");
        assert_eq!(value(Unit::Megahertz, 4405.4), "4405 MHz");
        assert_eq!(bytes(4 << 30), "4.0 GiB");
    }

    #[test]
    fn sparkline_scales() {
        assert_eq!(spark([0.0, 50.0, 100.0], 10, Some((0.0, 100.0))), "▁▅█");
        assert_eq!(
            spark([5.0, 5.0], 10, None),
            "▅▅",
            "steady values sit mid-height"
        );
        assert_eq!(
            spark([5500.0, 5501.0], 10, None),
            "▄▅",
            "noise stays near the middle"
        );
        assert_eq!(spark([0.0, 0.0], 10, None), "▁▁");
        assert_eq!(spark((0..30).map(f64::from), 4, None), "▁▃▆█");
        assert_eq!(spark([], 4, None), "");
        assert_eq!(spark_range(Unit::Percent), Some((0.0, 100.0)));
        assert_eq!(spark_range(Unit::Celsius), None);
    }

    #[test]
    fn support_roles() {
        assert_eq!(support(Some(&Support::Available)).1, Role::Ok);
        assert_eq!(support(None), ("unknown".to_owned(), Role::Missing));
    }

    #[test]
    fn utc_times() {
        assert_eq!(iso_utc(Timestamp(0)), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso_utc(Timestamp(1_767_225_600_000)),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(iso_utc(Timestamp(951_782_400_000)), "2000-02-29T00:00:00Z");
        assert_eq!(
            iso_utc(Timestamp(1_792_619_434_000)),
            "2026-10-21T21:50:34Z"
        );
    }
}
