use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Source;

/// Identifies a sensor within one provider, for example `"cpu0/package/temp"`.
/// Stable across polls and restarts on the same machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SensorId(pub String);

impl SensorId {
    /// Creates an id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// What a sensor measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SensorKind {
    /// Temperature.
    Temperature,
    /// Power draw.
    Power,
    /// Voltage.
    Voltage,
    /// Current.
    Current,
    /// Clock frequency.
    Clock,
    /// Utilization.
    Load,
    /// Fan speed.
    Fan,
    /// Memory in use.
    Memory,
    /// Accumulated energy.
    Energy,
}

impl SensorKind {
    /// The unit this kind of sensor is reported in.
    pub fn unit(self) -> Unit {
        match self {
            Self::Temperature => Unit::Celsius,
            Self::Power => Unit::Watt,
            Self::Voltage => Unit::Volt,
            Self::Current => Unit::Ampere,
            Self::Clock => Unit::Megahertz,
            Self::Load => Unit::Percent,
            Self::Fan => Unit::Rpm,
            Self::Memory => Unit::Byte,
            Self::Energy => Unit::Joule,
        }
    }
}

/// Unit of a sensor value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Unit {
    /// Degrees Celsius.
    Celsius,
    /// Watts.
    Watt,
    /// Volts.
    Volt,
    /// Amperes.
    Ampere,
    /// Megahertz.
    Megahertz,
    /// Percent, 0 to 100.
    Percent,
    /// Revolutions per minute.
    Rpm,
    /// Bytes.
    Byte,
    /// Joules.
    Joule,
}

/// Describes a sensor once; readings then refer to it by [`SensorId`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorDescriptor {
    /// Identifier, unique within the provider.
    pub id: SensorId,
    /// What it measures.
    pub kind: SensorKind,
    /// Unit of its readings.
    pub unit: Unit,
    /// Device it belongs to, for example `"cpu0"` or `"gpu1"`.
    pub device: String,
    /// Human-readable label, for example "Package temperature".
    pub label: String,
    /// Where its readings come from.
    pub source: Source,
    /// Limits the hardware itself reports, if any.
    #[serde(default, skip_serializing_if = "Limits::is_empty")]
    pub limits: Limits,
}

/// Limits a device reports for one of its sensors, in the sensor's unit,
/// for example hwmon's `temp1_max` and `temp1_crit`. Health checks prefer
/// them to generic thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Where the device considers the value high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<f64>,
    /// Where the device protects itself (throttles or shuts down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical: Option<f64>,
}

impl Limits {
    /// Neither limit is known.
    pub fn is_empty(&self) -> bool {
        self.high.is_none() && self.critical.is_none()
    }
}

/// Milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// The current wall-clock time.
    pub fn now() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        Self(ms)
    }
}

/// One value read from a sensor. A sensor that cannot be read is left out of
/// the result rather than reported as zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    /// Which sensor.
    pub id: SensorId,
    /// The value, in the descriptor's unit.
    pub value: f64,
    /// When it was read.
    pub at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_as_plain_strings() {
        let id = SensorId::new("cpu0/package/temp");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"cpu0/package/temp\"");
    }

    #[test]
    fn each_kind_has_a_unit() {
        assert_eq!(SensorKind::Temperature.unit(), Unit::Celsius);
        assert_eq!(SensorKind::Load.unit(), Unit::Percent);
        assert_eq!(SensorKind::Memory.unit(), Unit::Byte);
    }

    #[test]
    fn now_is_after_2020() {
        assert!(Timestamp::now().0 > 1_577_836_800_000);
    }
}
