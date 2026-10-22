//! The JSON every machine-facing interface answers with (the MCP tools and
//! the REST API), so they never drift apart. Every answer is a JSON object
//! (MCP's structured content must be one), keyed like the CLI's `--json`.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use surtr_core::{Hub, Sample, paths};
use surtr_model::{SensorId, SensorKind, Timestamp, Unit};
use surtr_store::Store;
use surtr_view::format;

/// Longest history range served.
pub const MAX_MINUTES: u64 = 7 * 24 * 60;
/// Most history buckets returned.
pub const MAX_BUCKETS: u64 = 1000;

/// Which sensors to return.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct SensorFilter {
    /// Only sensors of this device, for example `cpu0` or `gpu1`.
    pub device: Option<String>,
    /// Only sensors of this kind: `temperature`, `power`, `clock`, `load`,
    /// `fan`, `voltage`, `memory` and so on (as reported in `kind`).
    pub kind: Option<String>,
}

/// A history request.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct HistoryQuery {
    /// Sensor id as reported by the sensor list, for example
    /// `cpu0/package/temp`. Leave it out to list the recorded sensors.
    pub sensor: Option<String>,
    /// Provider of the sensor; needed only when two providers recorded the
    /// same id.
    pub provider: Option<String>,
    /// How far back to look, in minutes (default 60, at most 10080).
    pub minutes: Option<u64>,
    /// Bucket length in seconds (default: about 120 buckets over the range).
    pub bucket_seconds: Option<u64>,
}

/// One sensor with its current value.
#[derive(Debug, Serialize)]
pub struct Reading {
    id: String,
    provider: String,
    device: String,
    label: String,
    kind: SensorKind,
    unit: Unit,
    value: Option<f64>,
    /// The value with its unit, for example `52.0 °C`.
    text: Option<String>,
    at: Option<String>,
}

impl From<Sample> for Reading {
    fn from(s: Sample) -> Self {
        let d = s.descriptor;
        Self {
            text: s.value.map(|v| format::value(d.unit, v)),
            at: s.at.map(format::iso_utc),
            id: d.id.0,
            provider: d.source.provider,
            device: d.device,
            label: d.label,
            kind: d.kind,
            unit: d.unit,
            value: s.value,
        }
    }
}

/// JSON text with the home directory redacted.
pub fn data(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value)
        .map(|text| paths::redact(&text))
        .map_err(|e| e.to_string())
}

/// `value` with the home directory redacted, as JSON.
pub fn redacted(value: &impl Serialize) -> Result<Value, String> {
    serde_json::from_str(&data(value)?).map_err(|e| e.to_string())
}

fn kind_name(kind: SensorKind) -> String {
    match serde_json::to_value(kind) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// System information and the providers in use.
pub fn system(hub: &Hub) -> Value {
    let providers: Vec<&str> = hub.provider_ids();
    json!({ "system": hub.system_info(), "providers": providers })
}

/// The samples that pass `filter`: `{"sensors": [...]}`.
pub fn sensors(samples: Vec<Sample>, filter: &SensorFilter) -> Value {
    json!({ "sensors": readings(samples, filter) })
}

/// The samples that pass `filter`, as readings.
pub fn readings(samples: Vec<Sample>, filter: &SensorFilter) -> Vec<Reading> {
    samples
        .into_iter()
        .filter(|s| {
            filter
                .device
                .as_ref()
                .is_none_or(|d| s.descriptor.device == *d)
                && filter
                    .kind
                    .as_ref()
                    .is_none_or(|k| kind_name(s.descriptor.kind) == *k)
        })
        .map(Reading::from)
        .collect()
}

/// What Surtr can and cannot read: `{"capabilities": [...]}`.
pub fn capabilities(hub: &Hub) -> Value {
    json!({ "capabilities": hub.capabilities() })
}

/// Processes using each GPU: `{"processes": [...]}`.
pub fn gpu_processes(hub: &Hub) -> Value {
    json!({ "processes": hub.gpu_processes() })
}

/// Why each GPU is below its potential clock: `{"throttle": [...]}`.
pub fn throttle(hub: &Hub) -> Value {
    json!({ "throttle": hub.throttle_reasons() })
}

/// Recorded history from `db`: without a sensor, the recorded sensors
/// (`{"sensors": [...]}`); with one, avg/min/max buckets over the requested
/// range.
pub fn history(db: &Path, q: &HistoryQuery) -> Result<Value, String> {
    if !db.exists() {
        return Err("no history recorded yet; run `surtr serve` or `surtr record`".to_owned());
    }
    let store = Store::open(db).map_err(|e| e.to_string())?;
    let stored = store.sensors().map_err(|e| e.to_string())?;
    let Some(sensor) = &q.sensor else {
        let list: Vec<Value> = stored
            .iter()
            .map(|s| {
                json!({
                    "id": s.descriptor.id.0,
                    "provider": s.descriptor.source.provider,
                    "device": s.descriptor.device,
                    "label": s.descriptor.label,
                    "unit": s.descriptor.unit,
                    "samples": s.count,
                    "first": s.first.map(format::iso_utc),
                    "last": s.last.map(format::iso_utc),
                })
            })
            .collect();
        return Ok(json!({ "sensors": list }));
    };
    let matches: Vec<_> = stored
        .iter()
        .filter(|s| s.descriptor.id.0 == *sensor)
        .filter(|s| {
            q.provider
                .as_ref()
                .is_none_or(|p| s.descriptor.source.provider == *p)
        })
        .collect();
    let found = match matches.as_slice() {
        [one] => *one,
        [] => return Err(format!("no history for sensor {sensor:?}")),
        _ => {
            return Err(format!(
                "{sensor:?} was recorded by several providers; pass `provider`"
            ));
        }
    };
    let minutes = q.minutes.unwrap_or(60).clamp(1, MAX_MINUTES);
    let range_ms = minutes * 60_000;
    let bucket_ms = q
        .bucket_seconds
        .map_or(range_ms / 120, |s| s.saturating_mul(1000))
        .max(range_ms / MAX_BUCKETS)
        .max(1000);
    let to = Timestamp::now();
    let from = Timestamp(to.0.saturating_sub(range_ms));
    let buckets = store
        .history(
            &found.descriptor.source.provider,
            &SensorId(sensor.clone()),
            from,
            Timestamp(to.0 + 1),
            bucket_ms,
        )
        .map_err(|e| e.to_string())?;
    let points: Vec<Value> = buckets
        .iter()
        .map(|b| json!({ "at": format::iso_utc(b.at), "avg": b.avg, "min": b.min, "max": b.max, "count": b.count }))
        .collect();
    Ok(json!({
        "sensor": sensor,
        "provider": found.descriptor.source.provider,
        "label": found.descriptor.label,
        "unit": found.descriptor.unit,
        "from": format::iso_utc(from),
        "to": format::iso_utc(to),
        "bucket_seconds": bucket_ms / 1000,
        "buckets": points,
    }))
}
