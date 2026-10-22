use serde::{Deserialize, Serialize};

/// A feature a provider may or may not be able to deliver on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Feature {
    /// CPU vendor, brand, cores and clocks.
    CpuIdentity,
    /// CPU utilization.
    CpuLoad,
    /// CPU clock readings.
    CpuClock,
    /// CPU temperature.
    CpuTemperature,
    /// CPU power draw.
    CpuPower,
    /// CPU core voltage.
    CpuVoltage,
    /// Memory size and installed modules.
    MemoryInfo,
    /// Memory timings (SPD).
    MemoryTimings,
    /// GPU vendor, name and memory size.
    GpuIdentity,
    /// GPU temperature, power, clocks, load and memory use.
    GpuSensors,
    /// Per-process GPU usage.
    GpuProcesses,
    /// Why a GPU is below its potential clock.
    GpuThrottleReasons,
    /// Storage health (SMART / NVMe).
    StorageHealth,
    /// Motherboard fans and voltages.
    BoardSensors,
    /// Fan speed control.
    FanControl,
    /// CPU tuning (offsets, limits).
    CpuTuning,
    /// GPU tuning (power limit, clock offsets).
    GpuTuning,
}

impl Feature {
    /// Every feature, in display order.
    pub const ALL: &'static [Feature] = &[
        Self::CpuIdentity,
        Self::CpuLoad,
        Self::CpuClock,
        Self::CpuTemperature,
        Self::CpuPower,
        Self::CpuVoltage,
        Self::MemoryInfo,
        Self::MemoryTimings,
        Self::GpuIdentity,
        Self::GpuSensors,
        Self::GpuProcesses,
        Self::GpuThrottleReasons,
        Self::StorageHealth,
        Self::BoardSensors,
        Self::FanControl,
        Self::CpuTuning,
        Self::GpuTuning,
    ];
}

/// Whether a [`Feature`] works here, and if not, why. Front ends turn this into
/// an explicit "not available" state instead of a blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Support {
    /// Works now.
    Available,
    /// Works if Surtr runs elevated (admin or root), without a driver.
    NeedsElevation,
    /// Needs the optional privileged backend (for example PawnIO on Windows).
    NeedsPrivilegedBackend,
    /// The hardware isn't present, for example no GPU.
    NoHardware,
    /// Not possible with this provider or platform.
    Unsupported {
        /// Short explanation shown to the user.
        reason: String,
    },
}

/// How the user could make an unavailable feature work. Front ends show it;
/// only `surtr setup` runs the command, after the user confirms it (plan,
/// section 4 and issue #24).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remedy {
    /// What to do, as one sentence.
    pub text: String,
    /// A command to copy, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// An official page with more information or the download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Remedy {
    /// A remedy with only a description.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            command: None,
            url: None,
        }
    }

    /// Adds a command to copy.
    pub fn command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Adds a link.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// One feature and its support status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// The feature.
    pub feature: Feature,
    /// Its status.
    pub support: Support,
    /// How to make it work, when Surtr knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<Remedy>,
}

/// What a provider can and cannot do on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(pub Vec<Capability>);

impl Capabilities {
    /// Status of `feature`, or `None` if the provider doesn't mention it.
    pub fn support(&self, feature: Feature) -> Option<&Support> {
        self.get(feature).map(|c| &c.support)
    }

    /// The whole entry for `feature`.
    pub fn get(&self, feature: Feature) -> Option<&Capability> {
        self.0.iter().find(|c| c.feature == feature)
    }

    /// Adds or replaces the status of `feature`, without a remedy.
    pub fn set(&mut self, feature: Feature, support: Support) {
        self.set_with(feature, support, None);
    }

    /// Adds or replaces the status of `feature` and its remedy.
    pub fn set_with(&mut self, feature: Feature, support: Support, remedy: Option<Remedy>) {
        match self.0.iter_mut().find(|c| c.feature == feature) {
            Some(c) => {
                c.support = support;
                c.remedy = remedy;
            }
            None => self.0.push(Capability {
                feature,
                support,
                remedy,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_existing_entry() {
        let mut caps = Capabilities::default();
        caps.set(Feature::CpuTemperature, Support::NeedsPrivilegedBackend);
        caps.set(Feature::CpuTemperature, Support::Available);
        assert_eq!(caps.0.len(), 1);
        assert_eq!(
            caps.support(Feature::CpuTemperature),
            Some(&Support::Available)
        );
        assert_eq!(caps.support(Feature::FanControl), None);
    }

    #[test]
    fn support_is_tagged_by_status() {
        let json = serde_json::to_string(&Support::Unsupported {
            reason: "read-only".to_owned(),
        })
        .unwrap();
        assert_eq!(json, r#"{"status":"unsupported","reason":"read-only"}"#);
        let json = serde_json::to_string(&Support::Available).unwrap();
        assert_eq!(json, r#"{"status":"available"}"#);
    }
}
