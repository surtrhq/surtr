//! A deterministic fake [`Provider`] for tests, demos and front-end work on
//! machines without the real hardware.
//!
//! Each sensor follows a sine wave around a base value, advancing one step per
//! [`Provider::read_sensors`] call, and timestamps advance one second per step
//! from a fixed start. The same calls always give the same results. A provider
//! can also be loaded from a JSON fixture with [`MockProvider::from_json`].

use std::{
    f64::consts::TAU,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use surtr_model::{
    Arch, Capabilities, CpuInfo, CpuVendor, Feature, GpuInfo, GpuProcess, GpuProcessKind,
    GpuVendor, MemoryInfo, OsInfo, SensorDescriptor, SensorId, SensorKind, SensorReading, Source,
    Support, SystemInfo, ThrottleReason, ThrottleState, Timestamp,
};

use crate::{Provider, ProviderError};

/// First timestamp the mock reports: 2026-01-01T00:00:00Z, in milliseconds.
pub const START_MS: u64 = 1_767_225_600_000;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// One fake sensor: `base + amplitude * sin(2π * step / period)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MockSensor {
    /// Descriptor reported by [`Provider::sensors`].
    pub descriptor: SensorDescriptor,
    /// Center value.
    pub base: f64,
    /// Swing above and below `base`.
    pub amplitude: f64,
    /// Steps per full cycle; 0 means the value stays at `base`.
    pub period: u32,
}

impl MockSensor {
    fn value_at(&self, step: u64) -> f64 {
        if self.period == 0 {
            return self.base;
        }
        let phase = (step % u64::from(self.period)) as f64 / f64::from(self.period);
        self.base + self.amplitude * (TAU * phase).sin()
    }
}

/// Everything a [`MockProvider`] reports; this is also the JSON fixture format.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MockFixture {
    /// Returned by [`Provider::system_info`].
    pub system: SystemInfo,
    /// Sensors and how their values move.
    pub sensors: Vec<MockSensor>,
    /// Returned by [`Provider::capabilities`].
    pub capabilities: Capabilities,
    /// Returned by [`Provider::gpu_processes`].
    #[serde(default)]
    pub processes: Vec<GpuProcess>,
    /// Returned by [`Provider::throttle_reasons`].
    #[serde(default)]
    pub throttle: Vec<ThrottleState>,
}

/// A deterministic fake provider. See the [module docs](self).
#[derive(Debug)]
pub struct MockProvider {
    fixture: MockFixture,
    step: AtomicU64,
}

impl MockProvider {
    /// Provider id used in every [`Source`] this provider reports.
    pub const ID: &'static str = "mock";

    /// Creates a provider that reports `fixture`.
    pub fn new(fixture: MockFixture) -> Self {
        Self {
            fixture,
            step: AtomicU64::new(0),
        }
    }

    /// Loads a provider from a JSON [`MockFixture`].
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json).map(Self::new)
    }

    /// A plausible desktop: one 8-core CPU, 32 GiB of memory and one GPU.
    /// Names are clearly fake so the mock is never mistaken for real hardware.
    pub fn desktop() -> Self {
        let info = Source::new(Self::ID, "fixture");
        let system = SystemInfo {
            os: OsInfo {
                name: info.tag("Mock OS".to_owned()),
                version: info.tag("1.0".to_owned()),
                kernel: info.tag("1.0.0-mock".to_owned()),
                arch: info.tag(Arch::X86_64),
            },
            cpus: vec![CpuInfo {
                vendor: info.tag(CpuVendor::Other("Mock".to_owned())),
                brand: info.tag("Mock 8-Core Processor".to_owned()),
                physical_cores: info.tag(8),
                logical_cores: info.tag(16),
                base_clock_mhz: info.tag(3800.0),
                max_clock_mhz: info.tag(5000.0),
            }],
            memory: MemoryInfo {
                total_bytes: info.tag(32 * 1024 * 1024 * 1024),
                modules: Vec::new(),
            },
            gpus: vec![GpuInfo {
                device: "gpu0".to_owned(),
                vendor: info.tag(GpuVendor::Other("Mock".to_owned())),
                name: info.tag("Mock GPU 16 GB".to_owned()),
                vram_bytes: info.tag(16 * 1024 * 1024 * 1024),
                driver_version: info.tag("1.0".to_owned()),
            }],
        };

        let sensors = vec![
            sensor(
                "cpu0/package/temp",
                SensorKind::Temperature,
                "cpu0",
                "Package temperature",
                55.0,
                10.0,
                60,
            ),
            sensor(
                "cpu0/package/power",
                SensorKind::Power,
                "cpu0",
                "Package power",
                65.0,
                30.0,
                45,
            ),
            sensor(
                "cpu0/load",
                SensorKind::Load,
                "cpu0",
                "Total load",
                40.0,
                35.0,
                30,
            ),
            sensor(
                "cpu0/clock",
                SensorKind::Clock,
                "cpu0",
                "Effective clock",
                4200.0,
                600.0,
                20,
            ),
            sensor(
                "gpu0/temp",
                SensorKind::Temperature,
                "gpu0",
                "GPU temperature",
                60.0,
                8.0,
                90,
            ),
            sensor(
                "gpu0/power",
                SensorKind::Power,
                "gpu0",
                "Board power",
                180.0,
                80.0,
                40,
            ),
            sensor(
                "gpu0/memory/used",
                SensorKind::Memory,
                "gpu0",
                "VRAM used",
                6.0 * GIB,
                2.0 * GIB,
                120,
            ),
            sensor(
                "gpu0/fan",
                SensorKind::Fan,
                "gpu0",
                "Fan speed",
                1200.0,
                300.0,
                90,
            ),
        ];

        let mut capabilities = Capabilities::default();
        for feature in [
            Feature::CpuIdentity,
            Feature::CpuLoad,
            Feature::CpuClock,
            Feature::CpuTemperature,
            Feature::CpuPower,
            Feature::MemoryInfo,
            Feature::GpuIdentity,
            Feature::GpuSensors,
            Feature::GpuProcesses,
            Feature::GpuThrottleReasons,
        ] {
            capabilities.set(feature, Support::Available);
        }
        capabilities.set(Feature::MemoryTimings, Support::NeedsPrivilegedBackend);
        capabilities.set(
            Feature::FanControl,
            Support::Unsupported {
                reason: "the mock provider is read-only".to_owned(),
            },
        );

        let processes = vec![GpuProcess {
            device: "gpu0".to_owned(),
            pid: 4242,
            name: Some("mock-inference".to_owned()),
            kind: GpuProcessKind::Compute,
            used_memory_bytes: info.tag(4 * 1024 * 1024 * 1024),
        }];
        let throttle = vec![ThrottleState {
            device: "gpu0".to_owned(),
            reasons: vec![ThrottleReason::PowerCap],
            source: info.clone(),
        }];

        Self::new(MockFixture {
            system,
            sensors,
            capabilities,
            processes,
            throttle,
        })
    }

    /// The fixture this provider reports.
    pub fn fixture(&self) -> &MockFixture {
        &self.fixture
    }
}

fn sensor(
    id: &str,
    kind: SensorKind,
    device: &str,
    label: &str,
    base: f64,
    amplitude: f64,
    period: u32,
) -> MockSensor {
    MockSensor {
        descriptor: SensorDescriptor {
            id: SensorId::new(id),
            kind,
            unit: kind.unit(),
            device: device.to_owned(),
            label: label.to_owned(),
            source: Source::new(MockProvider::ID, "synthetic"),
        },
        base,
        amplitude,
        period,
    }
}

impl Provider for MockProvider {
    fn id(&self) -> &str {
        Self::ID
    }

    fn capabilities(&self) -> Capabilities {
        self.fixture.capabilities.clone()
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        Ok(self.fixture.system.clone())
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        Ok(self
            .fixture
            .sensors
            .iter()
            .map(|s| s.descriptor.clone())
            .collect())
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        let step = self.step.fetch_add(1, Ordering::Relaxed);
        let at = Timestamp(START_MS + step * 1000);
        Ok(self
            .fixture
            .sensors
            .iter()
            .map(|s| SensorReading {
                id: s.descriptor.id.clone(),
                value: s.value_at(step),
                at,
            })
            .collect())
    }

    fn gpu_processes(&self) -> Result<Vec<GpuProcess>, ProviderError> {
        Ok(self.fixture.processes.clone())
    }

    fn throttle_reasons(&self) -> Result<Vec<ThrottleState>, ProviderError> {
        Ok(self.fixture.throttle.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(p: &MockProvider) -> Vec<f64> {
        p.read_sensors().unwrap().iter().map(|r| r.value).collect()
    }

    #[test]
    fn same_calls_give_same_results() {
        let a = MockProvider::desktop();
        let b = MockProvider::desktop();
        for _ in 0..5 {
            assert_eq!(values(&a), values(&b));
        }
    }

    #[test]
    fn readings_advance_one_second_per_call() {
        let p = MockProvider::desktop();
        let first = p.read_sensors().unwrap()[0].at;
        let second = p.read_sensors().unwrap()[0].at;
        assert_eq!(first, Timestamp(START_MS));
        assert_eq!(second, Timestamp(START_MS + 1000));
    }

    #[test]
    fn readings_match_descriptors() {
        let p = MockProvider::desktop();
        let ids: Vec<_> = p.sensors().unwrap().into_iter().map(|d| d.id).collect();
        let read: Vec<_> = p
            .read_sensors()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, read);
    }

    #[test]
    fn values_stay_within_range() {
        let p = MockProvider::desktop();
        let ranges: Vec<_> = p
            .fixture()
            .sensors
            .iter()
            .map(|s| (s.base - s.amplitude, s.base + s.amplitude))
            .collect();
        for _ in 0..200 {
            for (value, (lo, hi)) in values(&p).into_iter().zip(&ranges) {
                assert!((*lo..=*hi).contains(&value), "{value} outside {lo}..={hi}");
            }
        }
    }

    #[test]
    fn every_value_is_tagged_as_mock() {
        let p = MockProvider::desktop();
        assert!(
            p.sensors()
                .unwrap()
                .iter()
                .all(|d| d.source.provider == MockProvider::ID)
        );
        let info = p.system_info().unwrap();
        assert_eq!(
            info.cpus[0].brand.as_ref().unwrap().source.provider,
            MockProvider::ID
        );
    }

    #[test]
    fn capabilities_cover_all_states_used_by_front_ends() {
        let caps = MockProvider::desktop().capabilities();
        assert_eq!(
            caps.support(Feature::CpuTemperature),
            Some(&Support::Available)
        );
        assert_eq!(
            caps.support(Feature::MemoryTimings),
            Some(&Support::NeedsPrivilegedBackend)
        );
        assert!(matches!(
            caps.support(Feature::FanControl),
            Some(Support::Unsupported { .. })
        ));
    }

    #[test]
    fn fixture_round_trips_through_json() {
        let original = MockProvider::desktop();
        let json = serde_json::to_string(original.fixture()).unwrap();
        let loaded = MockProvider::from_json(&json).unwrap();
        assert_eq!(loaded.fixture(), original.fixture());
        assert_eq!(values(&loaded), values(&original));
    }

    #[test]
    fn usable_as_trait_object() {
        let p: Box<dyn Provider> = Box::new(MockProvider::desktop());
        assert_eq!(p.id(), "mock");
    }
}
