//! `surtr export`: recorded history as CSV or JSON, one row per sensor per
//! time bucket (long format), for spreadsheets and scripts (plan 5.5).

use serde_json::{Value, json};
use surtr_model::Timestamp;
use surtr_store::{Bucket, Result, Store, StoredSensor};

use crate::history::iso_utc;

/// One sensor's buckets.
pub struct Series<'a> {
    /// The sensor as stored.
    pub sensor: &'a StoredSensor,
    /// Its buckets, oldest first.
    pub buckets: Vec<Bucket>,
}

/// Whether `s` matches any filter (case-insensitive substring of its id,
/// label, device or provider); no filters match everything.
pub fn matches(s: &StoredSensor, filters: &[String]) -> bool {
    let d = &s.descriptor;
    filters.is_empty()
        || filters.iter().any(|f| {
            let f = f.to_lowercase();
            [&d.id.0, &d.label, &d.device, &d.source.provider]
                .iter()
                .any(|t| t.to_lowercase().contains(&f))
        })
}

/// Reads the series to export.
pub fn collect<'a>(
    store: &Store,
    sensors: &'a [StoredSensor],
    filters: &[String],
    from: Timestamp,
    to: Timestamp,
    bucket_ms: u64,
) -> Result<Vec<Series<'a>>> {
    sensors
        .iter()
        .filter(|s| matches(s, filters))
        .map(|sensor| {
            let d = &sensor.descriptor;
            let buckets = store.history(&d.source.provider, &d.id, from, to, bucket_ms)?;
            Ok(Series { sensor, buckets })
        })
        .collect()
}

/// A CSV field, quoted when it contains a comma, quote or line break.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// Serde's name for an enum variant, for example `celsius`.
fn name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Long-format CSV with a header row.
pub fn csv(series: &[Series]) -> String {
    let mut out = String::from("time,provider,sensor,device,label,unit,avg,min,max,count\n");
    for s in series {
        let d = &s.sensor.descriptor;
        let fixed = [
            field(&d.source.provider),
            field(&d.id.0),
            field(&d.device),
            field(&d.label),
            field(&name(&d.unit)),
        ]
        .join(",");
        for b in &s.buckets {
            out.push_str(&format!(
                "{},{fixed},{},{},{},{}\n",
                iso_utc(b.at),
                b.avg,
                b.min,
                b.max,
                b.count
            ));
        }
    }
    out
}

/// A JSON array with one object per sensor and its points.
pub fn json(series: &[Series], bucket_ms: u64) -> Value {
    Value::Array(
        series
            .iter()
            .map(|s| {
                let points: Vec<Value> = s
                    .buckets
                    .iter()
                    .map(|b| {
                        json!({
                            "at": b.at,
                            "time": iso_utc(b.at),
                            "avg": b.avg,
                            "min": b.min,
                            "max": b.max,
                            "count": b.count,
                        })
                    })
                    .collect();
                json!({ "sensor": s.sensor.descriptor, "bucket_ms": bucket_ms, "points": points })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_when_needed() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
