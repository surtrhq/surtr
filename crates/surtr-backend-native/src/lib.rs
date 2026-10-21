//! Driverless OS backends for Surtr: the [`NativeProvider`] reads what the
//! operating system offers without a driver or elevation.
//!
//! Each OS has its own module; only one is compiled. Values the OS doesn't
//! report stay absent. Sensors arrive in milestone M2; until then this
//! provider reports identity information only.

use surtr_backend::{Provider, ProviderError};
use surtr_model::{Capabilities, Feature, SensorDescriptor, SensorReading, Support, SystemInfo};

// Shared by all OS modules; each OS build uses only a subset.
#[allow(dead_code)]
mod parse;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    pub const ID: &str = "unsupported";
    pub const SUPPORTED: bool = false;
    pub fn system_info() -> surtr_model::SystemInfo {
        surtr_model::SystemInfo::default()
    }
}

/// The driverless provider for the OS Surtr was built for.
#[derive(Debug)]
pub struct NativeProvider {
    _private: (),
}

impl NativeProvider {
    /// Creates the provider, or fails on an OS without a native backend.
    pub fn new() -> Result<Self, ProviderError> {
        if platform::SUPPORTED {
            Ok(Self { _private: () })
        } else {
            Err(ProviderError::Unavailable {
                reason: format!("no native backend for {}", std::env::consts::OS),
            })
        }
    }
}

impl Provider for NativeProvider {
    fn id(&self) -> &str {
        platform::ID
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::default();
        for &feature in Feature::ALL {
            caps.set(
                feature,
                Support::Unsupported {
                    reason: "not implemented yet".to_owned(),
                },
            );
        }
        caps.set(Feature::CpuIdentity, Support::Available);
        caps.set(Feature::MemoryInfo, Support::Available);
        caps
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        Ok(platform::system_info())
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        Ok(Vec::new())
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        Ok(Vec::new())
    }
}

#[cfg(all(test, any(windows, target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    // Runs against the real machine (CI runners included), so it only checks
    // what every supported OS must report.
    #[test]
    fn reads_this_machine() {
        let provider = NativeProvider::new().unwrap();
        let info = provider.system_info().unwrap();
        assert!(info.os.name.is_some(), "OS name missing: {info:#?}");
        assert!(info.os.arch.is_some(), "architecture missing: {info:#?}");
        assert!(!info.cpus.is_empty(), "no CPU packages: {info:#?}");
        let logical: u32 = info
            .cpus
            .iter()
            .filter_map(|c| c.logical_cores.as_ref().map(|f| f.value))
            .sum();
        assert!(logical > 0, "no logical processors: {info:#?}");
        let memory = info.memory.total_bytes.as_ref().map(|f| f.value);
        assert!(memory.unwrap_or(0) > 0, "memory size missing: {info:#?}");
        let id = provider.id().to_owned();
        assert_eq!(info.os.name.unwrap().source.provider, id);
    }
}
