//! Text for values and names, shared by every front end.

use surtr_model::{
    Arch, CpuVendor, Feature, GpuProcessKind, GpuVendor, Support, ThrottleReason, Unit,
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
    fn support_roles() {
        assert_eq!(support(Some(&Support::Available)).1, Role::Ok);
        assert_eq!(support(None), ("unknown".to_owned(), Role::Missing));
    }
}
