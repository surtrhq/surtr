//! Run records and the run journal (plan, M6).
//!
//! Every run is a JSON file, `<id>.json`, in the runs directory. It is
//! written before the run starts and rewritten (and synced to disk) as the
//! run goes, while the host holds `<id>.lock` locked. If the machine resets
//! or freezes during a run, the lock dies with the process: the next look at
//! the runs finds a record still marked running with its lock free, and
//! marks it interrupted. The timeline up to that moment is kept, since it is
//! often the best evidence of what went wrong.

use std::{
    fs::{self, File},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::runner::{Report, Watched};

/// Version of the record format.
pub const SCHEMA: u32 = 1;

/// Where a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Still running, or the host died without finishing the record.
    Running,
    /// Ended normally; [`Record::report`] says how.
    Done,
    /// The system stopped during the run (reset, freeze, power loss) or the
    /// host was killed.
    Interrupted,
}

/// One second of a run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    /// Milliseconds since the start.
    pub elapsed_ms: u64,
    /// Highest watched temperature, in °C.
    pub temp_c: Option<f64>,
    /// Iterations completed by all threads so far.
    pub iterations: u64,
}

/// A run, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Format version, [`SCHEMA`].
    pub schema: u32,
    /// Identifier, for example `20261022-234501-cpu`.
    pub id: String,
    /// Workload, for example `cpu`.
    pub workload: String,
    /// Where the run stands.
    pub state: State,
    /// Start, UTC ISO 8601.
    pub started: String,
    /// End, UTC ISO 8601, once known.
    pub ended: Option<String>,
    /// Planned length, in milliseconds.
    pub planned_ms: u64,
    /// Worker threads.
    pub threads: u32,
    /// Surtr version.
    pub surtr: String,
    /// The machine: CPU name and OS, as far as known.
    pub machine: Vec<String>,
    /// Temperatures watched and their limits.
    pub watched: Vec<Watched>,
    /// Wrong answers seen so far (the report has the details).
    pub errors_seen: u32,
    /// Once a second.
    pub timeline: Vec<Tick>,
    /// The result, once the run ended normally.
    pub report: Option<Report>,
}

fn json_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn lock_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.lock"))
}

/// Writes `record` atomically and syncs it, so a reset leaves the last
/// complete version on disk.
fn write(dir: &Path, record: &Record) -> io::Result<()> {
    let path = json_path(dir, &record.id);
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(record).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, &path)
}

/// An open run: the record on disk plus the lock that says the host lives.
#[derive(Debug)]
pub struct Journal {
    dir: PathBuf,
    lock: File,
    /// The record as last written.
    pub record: Record,
}

impl Journal {
    /// Writes the record and takes its lock, before the run starts.
    pub fn start(dir: &Path, record: Record) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let lock = File::create(lock_path(dir, &record.id))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        write(dir, &record)?;
        Ok(Self {
            dir: dir.to_owned(),
            lock,
            record,
        })
    }

    /// Rewrites the record with its current contents.
    pub fn save(&self) -> io::Result<()> {
        write(&self.dir, &self.record)
    }

    /// Stores the final record and releases the lock.
    pub fn finish(mut self, report: Report, ended: String) -> io::Result<Record> {
        self.record.state = State::Done;
        self.record.ended = Some(ended);
        self.record.report = Some(report);
        write(&self.dir, &self.record)?;
        drop(self.lock);
        let _ = fs::remove_file(lock_path(&self.dir, &self.record.id));
        Ok(self.record)
    }
}

/// Every record in `dir`, oldest first. Records left running by a host that
/// is gone become [`State::Interrupted`] (and are saved that way); the ids
/// of those are returned too.
pub fn load_all(dir: &Path) -> io::Result<(Vec<Record>, Vec<String>)> {
    let mut records = Vec::new();
    let mut interrupted = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((records, interrupted)),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // A record from a newer Surtr, or a damaged one, is skipped.
        let Some(mut record) = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
        else {
            continue;
        };
        if record.state == State::Running && host_gone(dir, &record.id) {
            record.state = State::Interrupted;
            write(dir, &record)?;
            let _ = fs::remove_file(lock_path(dir, &record.id));
            interrupted.push(record.id.clone());
        }
        records.push(record);
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((records, interrupted))
}

/// Whether the host of a running record is gone: its lock can be taken.
fn host_gone(dir: &Path, id: &str) -> bool {
    match File::open(lock_path(dir, id)) {
        Ok(file) => file.try_lock().is_ok(),
        Err(_) => true,
    }
}

/// The record with `id`, or the latest when `id` is `latest`.
pub fn find(dir: &Path, id: &str) -> io::Result<Option<Record>> {
    let (records, _) = load_all(dir)?;
    Ok(if id == "latest" {
        records.into_iter().last()
    } else {
        records.into_iter().find(|r| r.id == id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Outcome;

    fn record(id: &str) -> Record {
        Record {
            schema: SCHEMA,
            id: id.to_owned(),
            workload: "cpu".to_owned(),
            state: State::Running,
            started: "2026-10-22T23:45:01Z".to_owned(),
            ended: None,
            planned_ms: 60_000,
            threads: 8,
            surtr: "0.1.0".to_owned(),
            machine: vec!["Test CPU".to_owned()],
            watched: Vec::new(),
            errors_seen: 0,
            timeline: Vec::new(),
            report: None,
        }
    }

    fn report() -> Report {
        Report {
            outcome: Outcome::Passed,
            threads: 8,
            duration_ms: 60_000,
            iterations: vec![10; 8],
            failures: Vec::new(),
            peak_temp_c: Some(80.0),
            watched: Vec::new(),
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("surtr-records-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn finished_run_is_stored() {
        let d = dir("done");
        let mut j = Journal::start(&d, record("20261022-234501-cpu")).unwrap();
        j.record.timeline.push(Tick {
            elapsed_ms: 1000,
            temp_c: Some(70.0),
            iterations: 8,
        });
        j.save().unwrap();
        // While the host holds the lock the run is running, not interrupted.
        let (all, interrupted) = load_all(&d).unwrap();
        assert_eq!(all[0].state, State::Running);
        assert!(interrupted.is_empty());

        j.finish(report(), "2026-10-22T23:46:01Z".to_owned())
            .unwrap();
        let r = find(&d, "latest").unwrap().unwrap();
        assert_eq!(r.state, State::Done);
        assert_eq!(r.timeline.len(), 1);
        assert_eq!(r.report.unwrap().outcome, Outcome::Passed);
        assert!(!lock_path(&d, &r.id).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn run_left_behind_becomes_interrupted() {
        let d = dir("crash");
        let mut j = Journal::start(&d, record("20261022-234501-cpu")).unwrap();
        j.record.timeline.push(Tick {
            elapsed_ms: 31_000,
            temp_c: Some(93.0),
            iterations: 400,
        });
        j.save().unwrap();
        // The host dies: the lock goes, the files stay.
        drop(j);
        let (all, interrupted) = load_all(&d).unwrap();
        assert_eq!(interrupted, ["20261022-234501-cpu"]);
        assert_eq!(all[0].state, State::Interrupted);
        assert_eq!(
            all[0].timeline[0].temp_c,
            Some(93.0),
            "the timeline is kept"
        );
        // It is saved that way, once.
        let (_, again) = load_all(&d).unwrap();
        assert!(again.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_directory_means_no_runs() {
        let (all, interrupted) = load_all(&dir("none")).unwrap();
        assert!(all.is_empty() && interrupted.is_empty());
    }
}
