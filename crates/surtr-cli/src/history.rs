//! Durations, times and output for `surtr record` and `surtr history`.

use std::{fmt::Write, path::Path};

use serde_json::{Value, json};
use surtr_model::SensorDescriptor;
use surtr_store::{Bucket, StoredSensor};

use surtr_view::format::value as sensor_value;

/// Milliseconds in a duration such as `500ms`, `30s`, `10m`, `2h` or `7d`.
/// A bare number is seconds.
pub fn parse_duration_ms(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| format!("invalid duration {text:?}"))?;
    let factor = match unit {
        "ms" => 1,
        "" | "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => {
            return Err(format!(
                "invalid duration unit in {text:?} (use ms, s, m, h or d)"
            ));
        }
    };
    number
        .checked_mul(factor)
        .ok_or_else(|| format!("duration {text:?} is too long"))
}

pub use surtr_view::format::iso_utc;

/// The `surtr history` listing of recorded sensors.
pub fn sensors(path: &Path, sensors: &[StoredSensor]) -> String {
    let mut out = String::new();
    if sensors.is_empty() {
        let _ = writeln!(
            out,
            "Nothing recorded yet in {}. Run `surtr record` first.",
            path.display()
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{:<10}{:<28}{:>9}  Last sample",
        "Provider", "Sensor", "Samples"
    );
    for s in sensors {
        let last = s.last.map_or_else(|| "-".to_owned(), iso_utc);
        let _ = writeln!(
            out,
            "{:<10}{:<28}{:>9}  {last}",
            s.descriptor.source.provider, s.descriptor.id.0, s.count
        );
    }
    let _ = writeln!(out, "\nDatabase: {}", path.display());
    out
}

/// JSON for the sensor listing.
pub fn sensors_json(sensors: &[StoredSensor]) -> Value {
    sensors
        .iter()
        .map(|s| {
            json!({
                "sensor": s.descriptor,
                "count": s.count,
                "first": s.first,
                "last": s.last,
            })
        })
        .collect()
}

/// JSON for history points.
pub fn buckets_json(buckets: &[Bucket]) -> Value {
    buckets
        .iter()
        .map(|b| json!({ "at": b.at, "avg": b.avg, "min": b.min, "max": b.max, "count": b.count }))
        .collect()
}

/// The history of one sensor, oldest first.
pub fn render(sensor: &SensorDescriptor, buckets: &[Bucket], bucketed: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({}, {})",
        sensor.label, sensor.source.provider, sensor.id.0
    );
    if buckets.is_empty() {
        out.push_str("No samples in this time range.\n");
        return out;
    }
    for b in buckets {
        let value = sensor_value(sensor.unit, b.avg);
        if bucketed {
            let range = format!(
                "{} .. {}",
                sensor_value(sensor.unit, b.min),
                sensor_value(sensor.unit, b.max)
            );
            let _ = writeln!(
                out,
                "{}  {value:>14}  ({range}, {} samples)",
                iso_utc(b.at),
                b.count
            );
        } else {
            let _ = writeln!(out, "{}  {value:>14}", iso_utc(b.at));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("30"), Ok(30_000));
        assert_eq!(parse_duration_ms("10m"), Ok(600_000));
        assert_eq!(parse_duration_ms("2h"), Ok(7_200_000));
        assert_eq!(parse_duration_ms("7d"), Ok(604_800_000));
        assert_eq!(parse_duration_ms("0"), Ok(0));
        assert!(parse_duration_ms("5w").is_err());
        assert!(parse_duration_ms("m").is_err());
    }
}
