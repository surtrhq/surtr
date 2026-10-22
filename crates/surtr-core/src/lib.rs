//! Surtr's core. For now it holds the [`Hub`], which combines several
//! providers (the OS backend plus GPU backends) into one view. The polling
//! loop, history, safety limits and plugin host arrive in later milestones.

#![forbid(unsafe_code)]

use serde::Serialize;
use surtr_backend::{Provider, ProviderError};
use surtr_model::{
    Capabilities, Capability, Feature, GpuProcess, Remedy, SensorDescriptor, Support, SystemInfo,
    ThrottleState, Timestamp,
};

pub mod alerts;
pub mod config;
pub mod paths;

/// The value of a provider call, or `None` after logging why it failed.
fn logged<T>(p: &dyn Provider, what: &str, result: Result<T, ProviderError>) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!(provider = p.id(), "{what} failed: {err}");
            None
        }
    }
}

/// One sensor with its latest value. `value` is absent when the sensor
/// couldn't be read this time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    /// The sensor; `descriptor.source.provider` tells which provider owns it.
    #[serde(flatten)]
    pub descriptor: SensorDescriptor,
    /// Latest value, in the descriptor's unit.
    pub value: Option<f64>,
    /// When it was read.
    pub at: Option<Timestamp>,
}

/// Several providers seen as one.
///
/// Sensor ids are unique within a provider; across providers the pair
/// `(descriptor.source.provider, descriptor.id)` is unique. A provider whose
/// call fails is skipped for that call; logging of such failures arrives with
/// structured logging (plan, section 8.1).
#[derive(Default)]
pub struct Hub {
    providers: Vec<Box<dyn Provider>>,
    missing: Vec<(Vec<Feature>, String, Option<Remedy>)>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("providers", &self.provider_ids())
            .field("missing", &self.missing)
            .finish()
    }
}

/// Higher is better when providers disagree about a feature.
fn rank(support: &Support) -> u8 {
    match support {
        Support::Available => 4,
        Support::NeedsElevation => 3,
        Support::NeedsPrivilegedBackend => 2,
        Support::NoHardware => 1,
        Support::Unsupported { .. } => 0,
    }
}

impl Hub {
    /// An empty hub.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a provider.
    pub fn add(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    /// Records that a backend which would have provided `features` couldn't
    /// start, so [`Hub::capabilities`] can say why they are missing.
    pub fn add_missing(
        &mut self,
        features: &[Feature],
        reason: impl Into<String>,
        remedy: Option<Remedy>,
    ) {
        self.missing
            .push((features.to_vec(), reason.into(), remedy));
    }

    /// Ids of the providers, in the order added.
    pub fn provider_ids(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.id()).collect()
    }

    /// One snapshot: OS, CPUs and memory from the first provider that has
    /// them, GPUs from all providers.
    pub fn system_info(&self) -> SystemInfo {
        let mut out = SystemInfo::default();
        for info in self
            .providers
            .iter()
            .filter_map(|p| logged(p.as_ref(), "system info", p.system_info()))
        {
            if out.os.name.is_none() && info.os.name.is_some() {
                out.os = info.os;
            }
            if out.cpus.is_empty() {
                out.cpus = info.cpus;
            }
            if out.memory.total_bytes.is_none() {
                out.memory = info.memory;
            }
            out.gpus.extend(info.gpus);
        }
        out
    }

    /// Every feature's best status across providers. Features a missing
    /// backend would have provided are `Unsupported` with its reason unless
    /// another provider does better.
    pub fn capabilities(&self) -> Capabilities {
        let mut out = Capabilities::default();
        // A better status wins; at equal status, an entry with a remedy beats
        // one without.
        let mut offer = |c: Capability| {
            let better = match out.get(c.feature) {
                None => true,
                Some(current) => {
                    let (new, old) = (rank(&c.support), rank(&current.support));
                    new > old || (new == old && current.remedy.is_none() && c.remedy.is_some())
                }
            };
            if better {
                out.set_with(c.feature, c.support, c.remedy);
            }
        };
        for caps in self.providers.iter().map(|p| p.capabilities()) {
            for c in caps.0 {
                offer(c);
            }
        }
        for (features, reason, remedy) in &self.missing {
            for &feature in features {
                offer(Capability {
                    feature,
                    support: Support::Unsupported {
                        reason: reason.clone(),
                    },
                    remedy: remedy.clone(),
                });
            }
        }
        out
    }

    /// All sensors of all providers.
    pub fn sensors(&self) -> Vec<SensorDescriptor> {
        self.providers
            .iter()
            .filter_map(|p| logged(p.as_ref(), "sensor list", p.sensors()))
            .flatten()
            .collect()
    }

    /// Reads every provider once. Rate sensors such as CPU load need two
    /// reads some time apart before they have values.
    pub fn read(&self) -> Vec<Sample> {
        let mut out = Vec::new();
        for p in &self.providers {
            let Some(descriptors) = logged(p.as_ref(), "sensor list", p.sensors()) else {
                continue;
            };
            let readings = logged(p.as_ref(), "sensor read", p.read_sensors()).unwrap_or_default();
            out.extend(descriptors.into_iter().map(|descriptor| {
                let reading = readings.iter().find(|r| r.id == descriptor.id);
                Sample {
                    value: reading.map(|r| r.value),
                    at: reading.map(|r| r.at),
                    descriptor,
                }
            }));
        }
        out
    }

    /// GPU processes from all providers.
    pub fn gpu_processes(&self) -> Vec<GpuProcess> {
        self.providers
            .iter()
            .filter_map(|p| logged(p.as_ref(), "GPU processes", p.gpu_processes()))
            .flatten()
            .collect()
    }

    /// Throttle states from all providers.
    pub fn throttle_reasons(&self) -> Vec<ThrottleState> {
        self.providers
            .iter()
            .filter_map(|p| logged(p.as_ref(), "throttle reasons", p.throttle_reasons()))
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{ProviderError, mock::MockProvider};
    use surtr_model::{GpuInfo, GpuVendor, SensorReading, Source};

    /// A provider that only has GPUs, like a GPU backend.
    struct GpuOnly;

    impl Provider for GpuOnly {
        fn id(&self) -> &str {
            "gpu-only"
        }
        fn capabilities(&self) -> Capabilities {
            let mut caps = Capabilities::default();
            caps.set(Feature::GpuIdentity, Support::Available);
            caps.set(Feature::CpuTemperature, Support::NoHardware);
            caps
        }
        fn system_info(&self) -> Result<SystemInfo, ProviderError> {
            Ok(SystemInfo {
                gpus: vec![GpuInfo {
                    vendor: Source::new("gpu-only", "test").tag(GpuVendor::Nvidia),
                    ..GpuInfo::default()
                }],
                ..SystemInfo::default()
            })
        }
        fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
            Ok(Vec::new())
        }
        fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
            Err(ProviderError::Unavailable {
                reason: "test".to_owned(),
            })
        }
    }

    fn hub() -> Hub {
        let mut hub = Hub::new();
        hub.add(Box::new(MockProvider::desktop()));
        hub.add(Box::new(GpuOnly));
        hub
    }

    #[test]
    fn system_info_merges_gpus() {
        let info = hub().system_info();
        assert!(info.os.name.is_some());
        assert_eq!(info.cpus.len(), 1);
        assert_eq!(info.gpus.len(), 2);
    }

    #[test]
    fn best_capability_wins() {
        let mut hub = hub();
        hub.add_missing(
            &[Feature::GpuIdentity, Feature::GpuTuning],
            "no AMD library",
            Some(Remedy::new("Install the AMD driver").url("https://example.invalid")),
        );
        let caps = hub.capabilities();
        // Available beats a missing backend; the remedy only shows where the
        // missing backend's status is the best there is.
        assert!(caps.get(Feature::GpuIdentity).unwrap().remedy.is_none());
        assert_eq!(
            caps.get(Feature::GpuTuning)
                .unwrap()
                .remedy
                .as_ref()
                .unwrap()
                .text,
            "Install the AMD driver"
        );
        assert_eq!(
            caps.support(Feature::CpuTemperature),
            Some(&Support::Available)
        );
        assert_eq!(
            caps.support(Feature::GpuIdentity),
            Some(&Support::Available)
        );
        assert_eq!(
            caps.support(Feature::GpuTuning),
            Some(&Support::Unsupported {
                reason: "no AMD library".to_owned()
            })
        );
    }

    #[test]
    fn read_joins_values_and_keeps_unread_sensors() {
        let mut fixture = MockProvider::desktop().fixture().clone();
        fixture.sensors.truncate(2);
        let mut hub = Hub::new();
        hub.add(Box::new(MockProvider::new(fixture)));
        hub.add(Box::new(GpuOnly));
        let samples = hub.read();
        assert_eq!(samples.len(), 2);
        assert!(samples.iter().all(|s| s.value.is_some()));
    }

    #[test]
    fn processes_and_throttle_come_from_providers() {
        let hub = hub();
        assert_eq!(hub.gpu_processes()[0].pid, 4242);
        assert_eq!(hub.throttle_reasons()[0].device, "gpu0");
    }
}
