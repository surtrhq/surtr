//! Durations, times and output for `surtr record` and `surtr history`.

use std::{fmt::Write, path::Path};

use serde_json::{Value, json};
use surtr_model::{SensorDescriptor, Timestamp};
use surtr_store::{Bucket, StoredSensor};

use crate::render::sensor_value;

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

/// A timestamp as UTC ISO 8601, for example `2026-10-21T21:50:34Z`.
pub fn iso_utc(t: Timestamp) -> String {
    let secs = t.0 / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

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

    #[test]
    fn utc_times() {
        assert_eq!(iso_utc(Timestamp(0)), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso_utc(Timestamp(1_767_225_600_000)),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(iso_utc(Timestamp(951_782_400_000)), "2000-02-29T00:00:00Z");
        assert_eq!(
            iso_utc(Timestamp(1_792_619_434_000)),
            "2026-10-21T21:50:34Z"
        );
    }
}
