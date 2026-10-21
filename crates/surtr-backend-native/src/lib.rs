//! Driverless OS backends for Surtr: the [`NativeProvider`] reads what the
//! operating system offers without a driver or elevation.
//!
//! Each OS has its own module; only one is compiled. Values the OS doesn't
//! report stay absent.

use surtr_backend::{Provider, ProviderError};
use surtr_model::{
    Capabilities, Feature, SensorDescriptor, SensorId, SensorKind, SensorReading, Source, Support,
    SystemInfo, Timestamp,
};

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
    use surtr_model::{Capabilities, SensorDescriptor, SensorReading, SystemInfo};

    pub const ID: &str = "unsupported";
    pub const SUPPORTED: bool = false;

    pub fn system_info() -> SystemInfo {
        SystemInfo::default()
    }

    pub fn process_name(_pid: u32) -> Option<String> {
        None
    }

    #[derive(Debug)]
    pub struct State;

    impl State {
        pub fn new() -> Self {
            Self
        }
        pub fn capabilities(&self, _: &mut Capabilities) {}
        pub fn sensors(&self) -> Vec<SensorDescriptor> {
            Vec::new()
        }
        pub fn read(&self) -> Vec<SensorReading> {
            Vec::new()
        }
    }
}

/// A sensor descriptor whose unit follows from its kind.
#[allow(dead_code)]
fn descriptor(
    id: &str,
    kind: SensorKind,
    device: &str,
    label: &str,
    source: &Source,
) -> SensorDescriptor {
    SensorDescriptor {
        id: SensorId::new(id),
        kind,
        unit: kind.unit(),
        device: device.to_owned(),
        label: label.to_owned(),
        source: source.clone(),
    }
}

#[allow(dead_code)]
fn reading(id: SensorId, value: f64, at: Timestamp) -> SensorReading {
    SensorReading { id, value, at }
}

/// The executable name (without its path) of process `pid`, if the OS lets
/// this user see it.
pub fn process_name(pid: u32) -> Option<String> {
    platform::process_name(pid)
}

/// The driverless provider for the OS Surtr was built for.
///
/// Rate sensors such as CPU load compare two samples, so the first
/// [`Provider::read_sensors`] call right after [`NativeProvider::new`] may
/// leave them out; read again after a short interval.
#[derive(Debug)]
pub struct NativeProvider {
    state: platform::State,
}

impl NativeProvider {
    /// Creates the provider, or fails on an OS without a native backend.
    pub fn new() -> Result<Self, ProviderError> {
        if platform::SUPPORTED {
            Ok(Self {
                state: platform::State::new(),
            })
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
        // GPU features belong to the GPU providers, not to the OS backend.
        const GPU: [Feature; 5] = [
            Feature::GpuIdentity,
            Feature::GpuSensors,
            Feature::GpuProcesses,
            Feature::GpuThrottleReasons,
            Feature::GpuTuning,
        ];
        for &feature in Feature::ALL.iter().filter(|f| !GPU.contains(f)) {
            caps.set(
                feature,
                Support::Unsupported {
                    reason: "not implemented yet".to_owned(),
                },
            );
        }
        caps.set(Feature::CpuIdentity, Support::Available);
        caps.set(Feature::MemoryInfo, Support::Available);
        self.state.capabilities(&mut caps);
        caps
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        Ok(platform::system_info())
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        Ok(self.state.sensors())
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        Ok(self.state.read())
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

    #[test]
    fn names_own_process() {
        let name = process_name(std::process::id()).expect("own process name");
        // Linux and macOS truncate names (Linux to 15 bytes).
        assert!(name.starts_with("surtr_backend_n"), "{name}");
        assert!(!name.contains(['/', '\\']), "{name}");
    }

    // Every reading must belong to a declared sensor, and on Windows and Linux
    // the total CPU load must show up once two samples exist. The OS may mark
    // a single sample invalid (it is then left out), so a few reads are
    // allowed before the load must appear.
    #[test]
    fn readings_match_declared_sensors() {
        let provider = NativeProvider::new().unwrap();
        let declared: Vec<SensorId> = provider
            .sensors()
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        let mut load = None;
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let readings = provider.read_sensors().unwrap();
            for r in &readings {
                assert!(declared.contains(&r.id), "undeclared sensor {:?}", r.id);
                assert!(r.value.is_finite(), "{:?} is {}", r.id, r.value);
            }
            load = readings
                .iter()
                .find(|r| r.id.0 == "cpu/load")
                .map(|r| r.value);
            if load.is_some() {
                break;
            }
        }
        if cfg!(any(windows, target_os = "linux")) {
            let load = load.expect("no total CPU load in 3 reads");
            assert!((0.0..=100.0).contains(&load), "load {load}");
        }
    }
}
