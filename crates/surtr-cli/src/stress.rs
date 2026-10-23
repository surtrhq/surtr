//! `surtr stress cpu`: the host side of the built-in CPU test (plan, section
//! 5.3 and M6). The test itself runs in a `surtr worker` process; this module
//! picks the limits, watches the temperatures, and reports the result.

use std::{
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use surtr_core::{Hub, paths};
use surtr_model::Timestamp;
use surtr_stress::{
    event::Event,
    html::span,
    record::{self, Journal, Record, State, Tick},
    runner::{self, Outcome, Report, Watched},
};
use surtr_view::format::iso_utc;

/// Where run records live: `runs/` in the data directory.
pub fn runs_dir() -> Option<std::path::PathBuf> {
    paths::data_dir().map(|d| d.join("runs"))
}

/// One worker thread per logical CPU.
pub fn all_threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(1))
}

/// Records left running by a reset or crash, told to the user once.
fn report_interrupted(ids: &[String]) {
    for id in ids {
        eprintln!(
            "surtr: run {id} was interrupted: the system stopped during it (reset, freeze or \
             power loss), or Surtr was killed. `surtr runs report {id}` shows what it recorded."
        );
    }
}

/// A new run's id: the start time and the workload, unique in `dir`.
fn new_id(dir: &std::path::Path, workload: &str) -> String {
    let base = format!("{}-{workload}", crate::diag::stamp(Timestamp::now()));
    let mut id = base.clone();
    let mut n = 2;
    while dir.join(format!("{id}.json")).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// What the user asked for.
#[derive(Debug)]
pub struct Options {
    /// How long to run.
    pub duration: Duration,
    /// Longest allowed run (`stress.max_duration`).
    pub max_duration: Duration,
    /// Temperature limit in °C (`stress.max_temp_c`).
    pub max_temp_c: f64,
    /// Worker threads.
    pub threads: u32,
    /// Allow a short run without a temperature sensor.
    pub unmonitored: bool,
    /// The AI agent that asked for the run (`surtr runs confirm`).
    pub requested_by: Option<String>,
}

/// Reads temperatures and clocks through the hub.
struct HubSensors<'a> {
    hub: &'a Hub,
    watched: Vec<String>,
    clocks: Vec<(u32, f64)>,
}

impl runner::Sensors for HubSensors<'_> {
    fn temperatures(&mut self) -> Vec<(String, f64)> {
        let samples = self.hub.read();
        self.clocks = samples
            .iter()
            .filter_map(|s| {
                let cpu = s
                    .descriptor
                    .id
                    .0
                    .strip_prefix("cpu/thread")?
                    .strip_suffix("/clock")?
                    .parse()
                    .ok()?;
                Some((cpu, s.value?))
            })
            .collect();
        samples
            .into_iter()
            .filter(|s| self.watched.contains(&s.descriptor.id.0))
            .filter_map(|s| Some((s.descriptor.id.0, s.value?)))
            .collect()
    }

    fn clock(&mut self, cpu: u32) -> Option<f64> {
        self.clocks.iter().find(|(c, _)| *c == cpu).map(|(_, v)| *v)
    }
}

/// Runs the test; returns the report and the exit code (0 passed, 2 stopped
/// early, 3 errors found or the worker failed).
pub fn run(hub: &Hub, opts: &Options, on_start: impl FnOnce(&str)) -> Result<(Report, u8), String> {
    if opts.duration > opts.max_duration {
        return Err(format!(
            "{} is longer than the {} limit (stress.max_duration in the settings)",
            span(u64::try_from(opts.duration.as_millis()).unwrap_or(u64::MAX)),
            span(u64::try_from(opts.max_duration.as_millis()).unwrap_or(u64::MAX)),
        ));
    }
    let watched: Vec<Watched> = runner::watched(&hub.sensors(), opts.max_temp_c);
    let mut duration = opts.duration;
    if watched.is_empty() {
        if !opts.unmonitored {
            return Err(
                "no CPU temperature sensor to watch, so a stress test could overheat this \
                 machine unnoticed; `surtr caps` shows why it is missing. Pass --unmonitored \
                 for a 60-second check without temperature protection."
                    .to_owned(),
            );
        }
        duration = duration.min(runner::UNMONITORED_MAX);
        eprintln!(
            "surtr: warning: no CPU temperature sensor; running {} without temperature protection",
            span(u64::try_from(duration.as_millis()).unwrap_or(0))
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // A second Ctrl+C after the first falls through to the default.
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }
    let mut worker = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    worker.args([
        "worker",
        "cpu",
        "--threads",
        &opts.threads.to_string(),
        "--duration-ms",
        &duration.as_millis().to_string(),
    ]);
    let limits: Vec<String> = watched
        .iter()
        .map(|w| format!("{} {:.0} °C", w.label, w.stop_at))
        .collect();
    eprintln!(
        "Stressing {} threads for {}; stops at {}. Press Ctrl+C to stop.",
        opts.threads,
        span(u64::try_from(duration.as_millis()).unwrap_or(0)),
        if limits.is_empty() {
            "the time limit".to_owned()
        } else {
            limits.join(", ")
        }
    );
    // The journal goes to disk before the load starts (plan, M6), so a
    // reset during the run still leaves a record.
    let mut journal = match runs_dir() {
        Some(dir) => {
            if let Ok((_, interrupted)) = record::load_all(&dir) {
                report_interrupted(&interrupted);
            }
            let system = hub.system_info();
            let mut machine: Vec<String> = system
                .cpus
                .iter()
                .filter_map(|c| c.brand.as_ref().map(|b| b.value.clone()))
                .take(1)
                .collect();
            if let Some(os) = system.os.name.as_ref() {
                machine.push(os.value.clone());
            }
            let record = Record {
                schema: record::SCHEMA,
                id: new_id(&dir, "cpu"),
                workload: "cpu".to_owned(),
                state: State::Running,
                started: iso_utc(Timestamp::now()),
                ended: None,
                planned_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                threads: opts.threads,
                surtr: env!("CARGO_PKG_VERSION").to_owned(),
                machine,
                watched: watched.clone(),
                requested_by: opts.requested_by.clone(),
                errors_seen: 0,
                timeline: Vec::new(),
                report: None,
            };
            Some(
                Journal::start(&dir, record)
                    .map_err(|e| format!("could not record the run: {e}"))?,
            )
        }
        None => {
            eprintln!("surtr: warning: no data directory; this run won't be recorded");
            None
        }
    };
    if let (Some(j), Some(dir)) = (&journal, runs_dir()) {
        on_start(&j.record.id);
        // An agent may ask a run to stop (`stop_stress_test`) by leaving a
        // file; stopping is always safe.
        let stop = Arc::clone(&stop);
        let file = surtr_stress::request::stop_path(&dir, &j.record.id);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if file.exists() {
                    let _ = std::fs::remove_file(&file);
                    eprintln!("\nsurtr: stop requested through MCP");
                    stop.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
    let live = std::io::stderr().is_terminal();
    let mut sensors = HubSensors {
        hub,
        watched: watched.iter().map(|w| w.id.clone()).collect(),
        clocks: Vec::new(),
    };
    let report = runner::run(
        worker,
        duration,
        &watched,
        &mut sensors,
        &stop,
        |event, temp| {
            if let (true, Event::Progress { elapsed_ms, .. }) = (live, event) {
                let shown = temp.map_or_else(String::new, |t| format!(" · {t:.1} °C"));
                eprint!("\r  {}{shown}    ", span(*elapsed_ms));
                let _ = std::io::stderr().flush();
            }
            let Some(j) = journal.as_mut() else { return };
            match event {
                Event::Progress {
                    elapsed_ms,
                    iterations,
                } => {
                    j.record.timeline.push(Tick {
                        elapsed_ms: *elapsed_ms,
                        temp_c: temp,
                        iterations: iterations.iter().sum(),
                    });
                    // Every other second is enough to see what led up to a
                    // reset.
                    if j.record.timeline.len() % 2 == 0 {
                        let _ = j.save();
                    }
                }
                Event::Error { .. } => {
                    j.record.errors_seen += 1;
                    let _ = j.save();
                }
                Event::Started { .. } | Event::Finished { .. } => {}
            }
        },
    )?;
    if let Some(j) = journal {
        let id = j.record.id.clone();
        j.finish(report.clone(), iso_utc(Timestamp::now()))
            .map_err(|e| format!("could not save the run: {e}"))?;
        eprintln!("Saved as run {id}; `surtr runs report {id}` writes an HTML report.");
    }
    if live {
        eprintln!();
    }
    let code = match report.outcome {
        Outcome::Passed => 0,
        Outcome::TemperatureLimit { .. } | Outcome::Stopped => 2,
        Outcome::ErrorsFound | Outcome::WorkerFailed { .. } => 3,
    };
    Ok((report, code))
}

/// The report as text.
pub fn describe(r: &Report) -> String {
    let mut out = String::new();
    let verdict = match &r.outcome {
        Outcome::Passed => "PASSED: no errors".to_owned(),
        Outcome::ErrorsFound => format!(
            "ERRORS FOUND: the CPU gave {} wrong answer(s); it is not stable at these settings",
            r.failures.len()
        ),
        Outcome::TemperatureLimit {
            sensor,
            value,
            limit,
        } => format!("STOPPED: {sensor} reached {value:.1} °C (limit {limit:.0} °C)"),
        Outcome::Stopped => "STOPPED by you".to_owned(),
        Outcome::WorkerFailed { reason } => format!("FAILED: {reason}"),
    };
    let _ = writeln!(out, "{verdict}");
    let total: u64 = r.iterations.iter().sum();
    let _ = writeln!(
        out,
        "  {} threads · {} · {total} iterations{}",
        r.threads,
        span(r.duration_ms),
        r.peak_temp_c
            .map_or_else(String::new, |t| format!(" · peak {t:.1} °C")),
    );
    for f in &r.failures {
        let where_ = f
            .cpu
            .map_or_else(|| format!("thread {}", f.thread), |c| format!("CPU {c}"));
        let clock = f
            .clock_mhz
            .map_or_else(String::new, |c| format!(" at {c:.0} MHz"));
        let _ = writeln!(
            out,
            "  ! {where_}: wrong {} result after {}{clock}",
            f.kernel,
            span(f.elapsed_ms)
        );
    }
    out
}
