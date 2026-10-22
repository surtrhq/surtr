//! Sensor history in SQLite.
//!
//! Each sensor is stored once (keyed by provider and sensor id) and each value
//! as a `(sensor, time, value)` row. Queries can downsample on the fly into
//! fixed-size time buckets with average, minimum and maximum. Old samples are
//! removed with [`Store::prune`]. The default file location comes from
//! `surtr_core::paths::history_db`. Long-term rollups arrive when the daemon
//! records continuously (plan, sections 4 and 5.5).

#![forbid(unsafe_code)]

use std::{collections::HashMap, fmt, fs, path::Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use surtr_model::{SensorDescriptor, SensorId, SensorKind, Source, Timestamp, Unit};

const SCHEMA_VERSION: &str = "1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sensor (
    id       INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    sensor   TEXT NOT NULL,
    kind     TEXT NOT NULL,
    unit     TEXT NOT NULL,
    device   TEXT NOT NULL,
    label    TEXT NOT NULL,
    method   TEXT NOT NULL,
    UNIQUE (provider, sensor)
);
CREATE TABLE IF NOT EXISTS sample (
    sensor INTEGER NOT NULL REFERENCES sensor (id),
    at     INTEGER NOT NULL,
    value  REAL NOT NULL,
    PRIMARY KEY (sensor, at)
) WITHOUT ROWID;
";

/// A history database error.
#[derive(Debug)]
pub struct StoreError(String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "history database: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self(err.to_string())
    }
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, StoreError>;

/// A sensor known to the database, with the span of its stored samples.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSensor {
    /// The sensor as it was last recorded.
    pub descriptor: SensorDescriptor,
    /// Number of stored samples.
    pub count: u64,
    /// Oldest stored sample.
    pub first: Option<Timestamp>,
    /// Newest stored sample.
    pub last: Option<Timestamp>,
}

/// Samples in one time bucket. Without bucketing each sample is its own
/// bucket, so `avg == min == max` and `count == 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Start of the bucket.
    pub at: Timestamp,
    /// Average value.
    pub avg: f64,
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// Samples in the bucket.
    pub count: u64,
}

/// Serde's name for an enum variant, for example `"celsius"`.
fn name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(text: String) -> Option<T> {
    serde_json::from_value(Value::String(text)).ok()
}

/// The sensor history database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    ids: HashMap<(String, String), i64>,
}

impl Store {
    /// Opens (creating if needed) the database at `path`, including missing
    /// parent directories.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StoreError(format!("{}: {e}", parent.display())))?;
        }
        let conn = Connection::open(path)?;
        // Write-ahead logging lets readers query while a recorder writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match version.as_deref() {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                    [SCHEMA_VERSION],
                )?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(other) => {
                return Err(StoreError(format!(
                    "schema version {other} is not supported (expected {SCHEMA_VERSION})"
                )));
            }
        }
        Ok(Self {
            conn,
            ids: HashMap::new(),
        })
    }

    /// The row id of `d`, inserting or refreshing its description on first
    /// use in this session.
    fn sensor_id(&mut self, d: &SensorDescriptor) -> Result<i64> {
        let key = (d.source.provider.clone(), d.id.0.clone());
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let id = self.conn.query_row(
            "INSERT INTO sensor (provider, sensor, kind, unit, device, label, method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (provider, sensor) DO UPDATE SET
                 kind = excluded.kind, unit = excluded.unit, device = excluded.device,
                 label = excluded.label, method = excluded.method
             RETURNING id",
            params![
                key.0,
                key.1,
                name(&d.kind),
                name(&d.unit),
                d.device,
                d.label,
                d.source.method
            ],
            |r| r.get(0),
        )?;
        self.ids.insert(key, id);
        Ok(id)
    }

    /// Stores values in one transaction. Returns how many were stored.
    pub fn record<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a SensorDescriptor, Timestamp, f64)>,
    ) -> Result<usize> {
        let values: Vec<_> = values.into_iter().collect();
        let mut rows = Vec::with_capacity(values.len());
        for (d, at, value) in values {
            rows.push((self.sensor_id(d)?, at, value));
        }
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO sample (sensor, at, value) VALUES (?1, ?2, ?3)",
            )?;
            for (id, at, value) in &rows {
                let at = i64::try_from(at.0).unwrap_or(i64::MAX);
                insert.execute(params![id, at, value])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// Deletes samples older than `before`. Returns how many were deleted.
    pub fn prune(&mut self, before: Timestamp) -> Result<usize> {
        let before = i64::try_from(before.0).unwrap_or(i64::MAX);
        Ok(self
            .conn
            .execute("DELETE FROM sample WHERE at < ?1", [before])?)
    }

    /// Every known sensor with its sample count and time span.
    pub fn sensors(&self) -> Result<Vec<StoredSensor>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.provider, s.sensor, s.kind, s.unit, s.device, s.label, s.method,
                    COUNT(x.at), MIN(x.at), MAX(x.at)
             FROM sensor s LEFT JOIN sample x ON x.sensor = s.id
             GROUP BY s.id ORDER BY s.provider, s.id",
        )?;
        let rows = stmt.query_map([], |r| {
            let ts = |v: Option<i64>| v.map(|v| Timestamp(u64::try_from(v).unwrap_or(0)));
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                ts(r.get(8)?),
                ts(r.get(9)?),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (provider, sensor, kind, unit, device, label, method, count, first, last) = row?;
            // Rows written by a newer Surtr with unknown kinds are skipped.
            let (Some(kind), Some(unit)) = (parse::<SensorKind>(kind), parse::<Unit>(unit)) else {
                continue;
            };
            out.push(StoredSensor {
                descriptor: SensorDescriptor {
                    id: SensorId(sensor),
                    kind,
                    unit,
                    device,
                    label,
                    source: Source::new(provider, method),
                    limits: Default::default(),
                },
                count: u64::try_from(count).unwrap_or(0),
                first,
                last,
            });
        }
        Ok(out)
    }

    /// Samples of one sensor in `[from, to)`, grouped into buckets of
    /// `bucket_ms` milliseconds (1 returns every sample).
    pub fn history(
        &self,
        provider: &str,
        sensor: &SensorId,
        from: Timestamp,
        to: Timestamp,
        bucket_ms: u64,
    ) -> Result<Vec<Bucket>> {
        let i = |t: u64| i64::try_from(t).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(
            "SELECT (x.at / ?1) * ?1 AS t, AVG(x.value), MIN(x.value), MAX(x.value), COUNT(*)
             FROM sample x JOIN sensor s ON s.id = x.sensor
             WHERE s.provider = ?2 AND s.sensor = ?3 AND x.at >= ?4 AND x.at < ?5
             GROUP BY t ORDER BY t",
        )?;
        let rows = stmt.query_map(
            params![i(bucket_ms.max(1)), provider, sensor.0, i(from.0), i(to.0)],
            |r| {
                Ok(Bucket {
                    at: Timestamp(u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0)),
                    avg: r.get(1)?,
                    min: r.get(2)?,
                    max: r.get(3)?,
                    count: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> SensorDescriptor {
        SensorDescriptor {
            id: SensorId::new("cpu0/package/temp"),
            kind: SensorKind::Temperature,
            unit: Unit::Celsius,
            device: "cpu0".to_owned(),
            label: "Package temperature".to_owned(),
            source: Source::new("mock", "synthetic"),
            limits: Default::default(),
        }
    }

    fn filled() -> Store {
        let mut store = Store::in_memory().unwrap();
        let d = temp();
        // 0 s .. 5 s, one value per second: 50, 51, ..., 55.
        let values: Vec<_> = (0..6u64)
            .map(|s| (&d, Timestamp(1_000_000 + s * 1000), 50.0 + s as f64))
            .collect();
        assert_eq!(store.record(values).unwrap(), 6);
        store
    }

    #[test]
    fn raw_history() {
        let store = filled();
        let points = store
            .history(
                "mock",
                &temp().id,
                Timestamp(1_001_000),
                Timestamp(1_003_000),
                1,
            )
            .unwrap();
        let values: Vec<f64> = points.iter().map(|b| b.avg).collect();
        assert_eq!(values, [51.0, 52.0]);
        assert!(points.iter().all(|b| b.count == 1 && b.min == b.max));
    }

    #[test]
    fn bucketed_history() {
        let store = filled();
        let buckets = store
            .history(
                "mock",
                &temp().id,
                Timestamp(0),
                Timestamp(u64::MAX / 2),
                3000,
            )
            .unwrap();
        // 1_000_000 is a multiple of 1000 but not 3000: buckets start at
        // 999_000 (50, 51), 1_002_000 (52, 53, 54) and 1_005_000 (55).
        let summary: Vec<_> = buckets
            .iter()
            .map(|b| (b.at.0, b.count, b.min, b.max))
            .collect();
        assert_eq!(
            summary,
            [
                (999_000, 2, 50.0, 51.0),
                (1_002_000, 3, 52.0, 54.0),
                (1_005_000, 1, 55.0, 55.0)
            ]
        );
        assert_eq!(buckets[1].avg, 53.0);
    }

    #[test]
    fn sensors_round_trip_and_prune() {
        let mut store = filled();
        let sensors = store.sensors().unwrap();
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].descriptor, temp());
        assert_eq!(sensors[0].count, 6);
        assert_eq!(sensors[0].first, Some(Timestamp(1_000_000)));
        assert_eq!(sensors[0].last, Some(Timestamp(1_005_000)));
        assert_eq!(store.prune(Timestamp(1_004_000)).unwrap(), 4);
        assert_eq!(store.sensors().unwrap()[0].count, 2);
    }

    #[test]
    fn same_time_overwrites() {
        let mut store = Store::in_memory().unwrap();
        let d = temp();
        store.record([(&d, Timestamp(5), 1.0)]).unwrap();
        store.record([(&d, Timestamp(5), 2.0)]).unwrap();
        let points = store
            .history("mock", &d.id, Timestamp(0), Timestamp(10), 1)
            .unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].avg, 2.0);
    }

    #[test]
    fn file_database_reopens() {
        let dir = std::env::temp_dir().join(format!("surtr-store-{}", std::process::id()));
        let path = dir.join("nested/history.db");
        let _ = fs::remove_dir_all(&dir);
        {
            let mut store = Store::open(&path).unwrap();
            store.record([(&temp(), Timestamp(1), 42.0)]).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.sensors().unwrap()[0].count, 1);
        drop(store);
        let _ = fs::remove_dir_all(&dir);
    }
}
