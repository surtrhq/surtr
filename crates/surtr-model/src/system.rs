use serde::{Deserialize, Serialize};

use crate::Field;

/// A snapshot of the machine's hardware and OS. Fields are filled by
/// whichever providers can read them; anything else stays absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Operating system.
    pub os: OsInfo,
    /// One entry per CPU package (socket).
    pub cpus: Vec<CpuInfo>,
    /// System memory.
    pub memory: MemoryInfo,
    /// One entry per GPU.
    pub gpus: Vec<GpuInfo>,
}

/// Operating system details.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OsInfo {
    /// Product name, for example "Windows 11 Pro" or "Ubuntu 24.04".
    pub name: Field<String>,
    /// Version or build string.
    pub version: Field<String>,
    /// Kernel release (Linux, Darwin). On Windows the build is in `version`.
    pub kernel: Field<String>,
    /// Architecture the OS runs on (native, even if Surtr runs emulated).
    pub arch: Field<Arch>,
}

/// Instruction set architecture.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    /// x86-64 (AMD64).
    X86_64,
    /// ARM64 (AArch64).
    Aarch64,
    /// Anything else, by name.
    Other(String),
}

/// One CPU package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Vendor.
    pub vendor: Field<CpuVendor>,
    /// Marketing name as reported by the CPU or OS.
    pub brand: Field<String>,
    /// Physical cores in this package.
    pub physical_cores: Field<u32>,
    /// Logical processors (hardware threads) in this package.
    pub logical_cores: Field<u32>,
    /// Base clock in MHz.
    pub base_clock_mhz: Field<f64>,
    /// Maximum boost clock in MHz.
    pub max_clock_mhz: Field<f64>,
}

/// CPU vendor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuVendor {
    /// Intel.
    Intel,
    /// AMD.
    Amd,
    /// ARM Ltd. reference designs.
    Arm,
    /// Apple Silicon.
    Apple,
    /// Qualcomm.
    Qualcomm,
    /// Anything else, by name.
    Other(String),
}

/// System memory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryInfo {
    /// Total memory visible to the OS, in bytes.
    pub total_bytes: Field<u64>,
    /// Installed modules, where the platform reports them.
    pub modules: Vec<MemoryModule>,
}

/// One installed memory module. Serial numbers are deliberately not modeled.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryModule {
    /// Slot label, for example "DIMM_A2".
    pub slot: Field<String>,
    /// Module size in bytes.
    pub size_bytes: Field<u64>,
    /// Configured speed in MT/s.
    pub speed_mts: Field<u32>,
    /// Module manufacturer.
    pub manufacturer: Field<String>,
    /// Module part number.
    pub part_number: Field<String>,
}

/// One GPU.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Vendor.
    pub vendor: Field<GpuVendor>,
    /// Product name.
    pub name: Field<String>,
    /// Dedicated video memory in bytes (absent for unified-memory GPUs).
    pub vram_bytes: Field<u64>,
    /// Driver version string.
    pub driver_version: Field<String>,
}

/// GPU vendor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    /// NVIDIA.
    Nvidia,
    /// AMD.
    Amd,
    /// Intel.
    Intel,
    /// Apple.
    Apple,
    /// Anything else, by name.
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;

    #[test]
    fn empty_snapshot_has_no_values() {
        let info = SystemInfo::default();
        assert!(info.os.name.is_none());
        assert!(info.cpus.is_empty());
        assert!(info.memory.total_bytes.is_none());
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let src = Source::new("mock", "fixture");
        let info = SystemInfo {
            os: OsInfo {
                name: src.tag("Test OS".to_owned()),
                version: None,
                kernel: None,
                arch: src.tag(Arch::Aarch64),
            },
            cpus: vec![CpuInfo {
                vendor: src.tag(CpuVendor::Other("Test".to_owned())),
                logical_cores: src.tag(16),
                ..CpuInfo::default()
            }],
            ..SystemInfo::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: SystemInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, info);
    }
}
