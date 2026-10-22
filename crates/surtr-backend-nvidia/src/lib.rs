//! NVIDIA GPU backend for Surtr through NVML, which the NVIDIA driver installs.
//! NVML is loaded at run time and never shipped with Surtr (see NOTICE).
//!
//! Devices are named `nvidia0`, `nvidia1`, ... in NVML order. A sensor is
//! offered only if the GPU answered it when the provider was created, so
//! laptops without a fan reading, for example, simply have no fan sensor.

use surtr_backend::{Provider, ProviderError};
use surtr_model::{
    Capabilities, Feature, GpuInfo, GpuProcess, GpuProcessKind, GpuVendor, SensorDescriptor,
    SensorId, SensorKind, SensorReading, Source, Support, SystemInfo, ThrottleReason,
    ThrottleState, Timestamp, Unit,
};

mod ffi;

use ffi::{Clock, Device, Nvml};

/// What one NVIDIA sensor reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    Temperature,
    Power,
    PowerLimit,
    Load,
    MemoryLoad,
    GraphicsClock,
    MemoryClock,
    Fan,
    MemoryUsed,
}

impl Probe {
    const ALL: [Probe; 9] = [
        Self::Temperature,
        Self::Power,
        Self::PowerLimit,
        Self::Load,
        Self::MemoryLoad,
        Self::GraphicsClock,
        Self::MemoryClock,
        Self::Fan,
        Self::MemoryUsed,
    ];

    fn read(self, nvml: &Nvml, device: Device) -> Option<f64> {
        let mw = |p: u32| f64::from(p) / 1000.0;
        match self {
            Self::Temperature => nvml.temperature(device).map(f64::from),
            Self::Power => nvml.power_mw(device).map(mw),
            Self::PowerLimit => nvml.power_limit_mw(device).map(mw),
            Self::Load => nvml.utilization(device).map(|(g, _)| f64::from(g)),
            Self::MemoryLoad => nvml.utilization(device).map(|(_, m)| f64::from(m)),
            Self::GraphicsClock => nvml.clock(device, Clock::Graphics).map(f64::from),
            Self::MemoryClock => nvml.clock(device, Clock::Memory).map(f64::from),
            Self::Fan => nvml.fan_percent(device).map(f64::from),
            Self::MemoryUsed => nvml.memory(device).map(|(_, used)| used as f64),
        }
    }

    /// `(id suffix, kind, unit, label)`.
    fn describe(self) -> (&'static str, SensorKind, Unit, &'static str) {
        use SensorKind as K;
        match self {
            Self::Temperature => ("temp", K::Temperature, Unit::Celsius, "GPU temperature"),
            Self::Power => ("power", K::Power, Unit::Watt, "Board power"),
            Self::PowerLimit => ("power_limit", K::Power, Unit::Watt, "Power limit"),
            Self::Load => ("load", K::Load, Unit::Percent, "GPU load"),
            Self::MemoryLoad => (
                "memory_load",
                K::Load,
                Unit::Percent,
                "Memory controller load",
            ),
            Self::GraphicsClock => ("clock", K::Clock, Unit::Megahertz, "Graphics clock"),
            Self::MemoryClock => ("memory_clock", K::Clock, Unit::Megahertz, "Memory clock"),
            Self::Fan => ("fan", K::Fan, Unit::Percent, "Fan speed"),
            Self::MemoryUsed => ("memory/used", K::Memory, Unit::Byte, "VRAM used"),
        }
    }
}

/// Decodes NVML's clock event (throttle) reason bits.
pub fn decode_throttle(bits: u64) -> Vec<ThrottleReason> {
    const BITS: [(u64, ThrottleReason); 9] = [
        (0x1, ThrottleReason::Idle),
        (0x2, ThrottleReason::ApplicationClocks),
        (0x4, ThrottleReason::PowerCap),
        (0x8, ThrottleReason::HardwareSlowdown),
        (0x10, ThrottleReason::SyncBoost),
        (0x20, ThrottleReason::SoftwareThermal),
        (0x40, ThrottleReason::HardwareThermal),
        (0x80, ThrottleReason::HardwarePowerBrake),
        (0x100, ThrottleReason::DisplayClocks),
    ];
    BITS.iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|&(_, reason)| reason)
        .collect()
}

struct Gpu {
    handle: Device,
    name: String,
    probes: Vec<(SensorDescriptor, Probe)>,
}

/// The NVIDIA provider. Creating it loads NVML; it fails if NVML is missing
/// or no NVIDIA GPU is present.
pub struct NvidiaProvider {
    nvml: Nvml,
    gpus: Vec<Gpu>,
}

// SAFETY: device handles are opaque NVML pointers, and NVML is documented as
// thread-safe; `Nvml` itself is Send + Sync.
#[allow(unsafe_code)]
unsafe impl Send for NvidiaProvider {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for NvidiaProvider {}

impl std::fmt::Debug for NvidiaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NvidiaProvider")
            .field("gpus", &self.gpus.len())
            .finish()
    }
}

impl NvidiaProvider {
    /// Provider id used in every [`Source`] it reports.
    pub const ID: &'static str = "nvidia";

    /// Loads NVML and discovers GPUs and the sensors each one answers.
    pub fn new() -> Result<Self, ProviderError> {
        let nvml = Nvml::load().map_err(|reason| ProviderError::Unavailable { reason })?;
        let source = Source::new(Self::ID, "nvml");
        let gpus: Vec<Gpu> = (0..nvml.device_count())
            .filter_map(|i| {
                let handle = nvml.device(i)?;
                let device = format!("nvidia{i}");
                let probes = Probe::ALL
                    .into_iter()
                    .filter(|p| p.read(&nvml, handle).is_some())
                    .map(|p| {
                        let (suffix, kind, unit, label) = p.describe();
                        let descriptor = SensorDescriptor {
                            id: SensorId::new(format!("{device}/{suffix}")),
                            kind,
                            unit,
                            device: device.clone(),
                            label: label.to_owned(),
                            source: source.clone(),
                            limits: Default::default(),
                        };
                        (descriptor, p)
                    })
                    .collect();
                Some(Gpu {
                    handle,
                    name: device,
                    probes,
                })
            })
            .collect();
        if gpus.is_empty() {
            return Err(ProviderError::Unavailable {
                reason: "NVML found no NVIDIA GPU".to_owned(),
            });
        }
        Ok(Self { nvml, gpus })
    }
}

impl Provider for NvidiaProvider {
    fn id(&self) -> &str {
        Self::ID
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::default();
        caps.set(Feature::GpuIdentity, Support::Available);
        caps.set(Feature::GpuSensors, Support::Available);
        let throttle = self
            .gpus
            .iter()
            .any(|g| self.nvml.throttle_bits(g.handle).is_some());
        if throttle {
            caps.set(Feature::GpuThrottleReasons, Support::Available);
        }
        if self.nvml.has_processes() {
            caps.set(Feature::GpuProcesses, Support::Available);
        }
        caps
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        let src = Source::new(Self::ID, "nvml");
        let driver = self.nvml.driver_version();
        let gpus = self
            .gpus
            .iter()
            .map(|g| GpuInfo {
                device: g.name.clone(),
                vendor: src.tag(GpuVendor::Nvidia),
                name: self.nvml.name(g.handle).and_then(|n| src.tag(n)),
                vram_bytes: self
                    .nvml
                    .memory(g.handle)
                    .and_then(|(total, _)| src.tag(total)),
                driver_version: driver.clone().and_then(|d| src.tag(d)),
            })
            .collect();
        Ok(SystemInfo {
            gpus,
            ..SystemInfo::default()
        })
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        Ok(self
            .gpus
            .iter()
            .flat_map(|g| g.probes.iter().map(|(d, _)| d.clone()))
            .collect())
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        let at = Timestamp::now();
        Ok(self
            .gpus
            .iter()
            .flat_map(|g| {
                g.probes.iter().filter_map(move |(d, probe)| {
                    Some(SensorReading {
                        id: d.id.clone(),
                        value: probe.read(&self.nvml, g.handle)?,
                        at,
                    })
                })
            })
            .collect())
    }

    fn gpu_processes(&self) -> Result<Vec<GpuProcess>, ProviderError> {
        let src = Source::new(Self::ID, "nvml");
        let mut out = Vec::new();
        for g in &self.gpus {
            for (kind, compute) in [
                (GpuProcessKind::Compute, true),
                (GpuProcessKind::Graphics, false),
            ] {
                for (pid, used) in self.nvml.processes(g.handle, compute).unwrap_or_default() {
                    out.push(GpuProcess {
                        device: g.name.clone(),
                        pid,
                        name: None,
                        kind,
                        used_memory_bytes: used.and_then(|u| src.tag(u)),
                    });
                }
            }
        }
        Ok(out)
    }

    fn throttle_reasons(&self) -> Result<Vec<ThrottleState>, ProviderError> {
        let src = Source::new(Self::ID, "nvml");
        Ok(self
            .gpus
            .iter()
            .filter_map(|g| {
                let bits = self.nvml.throttle_bits(g.handle)?;
                Some(ThrottleState {
                    device: g.name.clone(),
                    reasons: decode_throttle(bits),
                    source: src.clone(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_bits_decode() {
        assert!(decode_throttle(0).is_empty());
        assert_eq!(decode_throttle(0x1), [ThrottleReason::Idle]);
        assert_eq!(
            decode_throttle(0x4 | 0x20),
            [ThrottleReason::PowerCap, ThrottleReason::SoftwareThermal]
        );
        // Unknown future bits are ignored rather than misreported.
        assert!(decode_throttle(1 << 40).is_empty());
    }

    // Runs only where an NVIDIA driver is installed (not on CI runners).
    #[test]
    fn reads_nvidia_gpus_if_present() {
        let Ok(provider) = NvidiaProvider::new() else {
            eprintln!("no NVIDIA GPU or NVML here; skipping");
            return;
        };
        let info = provider.system_info().unwrap();
        assert!(!info.gpus.is_empty());
        assert!(info.gpus[0].name.is_some(), "{info:#?}");
        let declared: Vec<_> = provider
            .sensors()
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert!(!declared.is_empty());
        for r in provider.read_sensors().unwrap() {
            assert!(declared.contains(&r.id));
            assert!(r.value.is_finite());
        }
        provider.gpu_processes().unwrap();
        provider.throttle_reasons().unwrap();
    }
}
