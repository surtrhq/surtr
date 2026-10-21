//! Plain-text output. Styling (color, tables, charts) comes with the CLI/TUI
//! quality pass in M3; this keeps the layout simple and stable for now.

use std::fmt::Write;

use surtr_model::{Arch, Capabilities, CpuVendor, Feature, Field, GpuVendor, Support, SystemInfo};

const MISSING: &str = "not available";

fn show<T>(field: &Field<T>, fmt: impl Fn(&T) -> String) -> String {
    field
        .as_ref()
        .map_or_else(|| MISSING.to_owned(), |f| fmt(&f.value))
}

fn text(field: &Field<String>) -> String {
    show(field, Clone::clone)
}

fn gib(bytes: &u64) -> String {
    format!("{:.1} GiB", *bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn mhz(value: &f64) -> String {
    format!("{value:.0} MHz")
}

fn arch(arch: &Arch) -> String {
    match arch {
        Arch::X86_64 => "x86-64".to_owned(),
        Arch::Aarch64 => "ARM64".to_owned(),
        Arch::Other(name) => name.clone(),
    }
}

fn cpu_vendor(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Intel => "Intel".to_owned(),
        CpuVendor::Amd => "AMD".to_owned(),
        CpuVendor::Arm => "Arm".to_owned(),
        CpuVendor::Apple => "Apple".to_owned(),
        CpuVendor::Qualcomm => "Qualcomm".to_owned(),
        CpuVendor::Other(name) => name.clone(),
    }
}

fn gpu_vendor(vendor: &GpuVendor) -> String {
    match vendor {
        GpuVendor::Nvidia => "NVIDIA".to_owned(),
        GpuVendor::Amd => "AMD".to_owned(),
        GpuVendor::Intel => "Intel".to_owned(),
        GpuVendor::Apple => "Apple".to_owned(),
        GpuVendor::Other(name) => name.clone(),
    }
}

fn row(out: &mut String, label: &str, value: &str) {
    // Writing to a String can't fail.
    let _ = writeln!(out, "  {label:<14}{value}");
}

/// The `surtr info` report.
pub fn info(provider: &str, system: &SystemInfo) -> String {
    let mut out = String::new();

    out.push_str("OS\n");
    row(&mut out, "Name", &text(&system.os.name));
    row(&mut out, "Version", &text(&system.os.version));
    if system.os.kernel.is_some() {
        row(&mut out, "Kernel", &text(&system.os.kernel));
    }
    row(&mut out, "Architecture", &show(&system.os.arch, arch));

    for (i, cpu) in system.cpus.iter().enumerate() {
        let _ = writeln!(out, "\nCPU {i}");
        row(&mut out, "Name", &text(&cpu.brand));
        row(&mut out, "Vendor", &show(&cpu.vendor, cpu_vendor));
        row(
            &mut out,
            "Cores",
            &show(&cpu.physical_cores, u32::to_string),
        );
        row(
            &mut out,
            "Threads",
            &show(&cpu.logical_cores, u32::to_string),
        );
        row(&mut out, "Base clock", &show(&cpu.base_clock_mhz, mhz));
        row(&mut out, "Max clock", &show(&cpu.max_clock_mhz, mhz));
    }

    out.push_str("\nMemory\n");
    row(&mut out, "Total", &show(&system.memory.total_bytes, gib));

    for (i, gpu) in system.gpus.iter().enumerate() {
        let _ = writeln!(out, "\nGPU {i}");
        row(&mut out, "Name", &text(&gpu.name));
        row(&mut out, "Vendor", &show(&gpu.vendor, gpu_vendor));
        row(&mut out, "Memory", &show(&gpu.vram_bytes, gib));
        row(&mut out, "Driver", &text(&gpu.driver_version));
    }

    let _ = writeln!(
        out,
        "\nProvider: {provider}. Run with --json to see where each value came from."
    );
    out
}

fn feature_label(feature: Feature) -> &'static str {
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

fn support_label(support: Option<&Support>) -> String {
    match support {
        Some(Support::Available) => "available".to_owned(),
        Some(Support::NeedsElevation) => "needs admin/root".to_owned(),
        Some(Support::NeedsPrivilegedBackend) => "needs privileged backend".to_owned(),
        Some(Support::NoHardware) => "no hardware".to_owned(),
        Some(Support::Unsupported { reason }) => format!("unsupported ({reason})"),
        None => "unknown".to_owned(),
    }
}

/// The `surtr caps` report: every feature, including ones the provider
/// doesn't mention.
pub fn caps(provider: &str, capabilities: &Capabilities) -> String {
    let mut out = String::new();
    for &feature in Feature::ALL {
        let _ = writeln!(
            out,
            "{:<22}{}",
            feature_label(feature),
            support_label(capabilities.support(feature))
        );
    }
    let _ = writeln!(out, "\nProvider: {provider}.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{Provider, mock::MockProvider};

    #[test]
    fn info_shows_values_and_missing_fields() {
        let mock = MockProvider::desktop();
        let mut system = mock.system_info().unwrap();
        system.cpus[0].max_clock_mhz = None;
        let out = info("mock", &system);
        assert!(out.contains("  Name          Mock 8-Core Processor\n"));
        assert!(out.contains("  Threads       16\n"));
        assert!(out.contains("  Base clock    3800 MHz\n"));
        assert!(out.contains("  Max clock     not available\n"));
        assert!(out.contains("  Total         32.0 GiB\n"));
        assert!(out.contains("GPU 0"));
        assert!(
            out.ends_with("Provider: mock. Run with --json to see where each value came from.\n")
        );
    }

    #[test]
    fn caps_lists_every_feature() {
        let out = caps("mock", &MockProvider::desktop().capabilities());
        assert_eq!(out.lines().count(), Feature::ALL.len() + 2);
        assert!(out.contains("CPU identity          available\n"));
        assert!(out.contains("Memory timings        needs privileged backend\n"));
        assert!(
            out.contains("Fan control           unsupported (the mock provider is read-only)\n")
        );
        assert!(out.contains("GPU tuning            unknown\n"));
    }
}
