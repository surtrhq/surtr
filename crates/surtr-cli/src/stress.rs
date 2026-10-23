//! `surtr stress cpu`: the host side of the built-in CPU test (plan, section
//! 5.3 and M6). The test itself runs in a `surtr worker` process; this module
//! picks the limits, watches the temperatures, and reports the result.

use std::{
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    process::Command,
    sync::{
        Arc, Mutex, PoisonError,
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
    runner::{self, CoreResult, Outcome, Report, Watched},
};
use surtr_tui::fire::{self, Ending, Live, ThreadLive};
use surtr_view::format::iso_utc;

/// Whether to show the flame view: on a terminal, unless `--plain`,
/// `NO_COLOR` or a dumb terminal says otherwise.
pub fn animate(plain: bool) -> bool {
    !plain
        && std::io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

/// Where run records live: `runs/` in the data directory.
pub fn runs_dir() -> Option<std::path::PathBuf> {
    paths::data_dir().map(|d| d.join("runs"))
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
    /// Worker threads (default: every logical CPU), placed one per physical
    /// core before SMT siblings.
    pub threads: Option<u32>,
    /// Exactly these logical CPUs, one thread each (overrides `threads`).
    pub cpus: Option<Vec<u32>>,
    /// Use SMT siblings; when false, at most one thread per physical core.
    pub smt: bool,
    /// Test one physical core at a time, this long each (`--per-core`).
    pub per_core: Option<Duration>,
    /// With `per_core`, load both SMT siblings of each core.
    pub siblings: bool,
    /// Steady load, or switching on and off.
    pub pattern: surtr_stress::pattern::Pattern,
    /// Which computations to run.
    pub kernels: surtr_stress::Kernels,
    /// Allow a short run without a temperature sensor.
    pub unmonitored: bool,
    /// The AI agent that asked for the run (`surtr runs confirm`).
    pub requested_by: Option<String>,
    /// Show the live flame view (on a terminal, unless --plain, --json or
    /// NO_COLOR).
    pub animate: bool,
}

/// Reads temperatures and clocks through the hub.
struct HubSensors<'a> {
    hub: &'a Hub,
    watched: Vec<String>,
    clocks: Vec<(u32, f64)>,
    /// The highest clock seen per CPU since last cleared.
    peaks: Vec<(u32, f64)>,
    /// The live view and each worker thread's CPU, when it is shown.
    view: Option<(Arc<Mutex<Live>>, Vec<u32>)>,
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
        for &(cpu, mhz) in &self.clocks {
            match self.peaks.iter_mut().find(|(c, _)| *c == cpu) {
                Some(p) => p.1 = p.1.max(mhz),
                None => self.peaks.push((cpu, mhz)),
            }
        }
        if let Some((live, cpus)) = &self.view {
            let mut live = live.lock().unwrap_or_else(PoisonError::into_inner);
            for (t, cpu) in live.threads.iter_mut().zip(cpus) {
                t.clock_mhz = self.clocks.iter().find(|(c, _)| c == cpu).map(|(_, v)| *v);
            }
        }
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

/// The run's test name: `cpu`, with `-per-core` and `-transient` as
/// they apply, so runs of different kinds don't compare silently.
fn workload_name(
    per_core: bool,
    pattern: surtr_stress::pattern::Pattern,
    kernels: surtr_stress::Kernels,
) -> String {
    let mut name = "cpu".to_owned();
    if kernels != surtr_stress::Kernels::Mixed {
        name.push('-');
        name.push_str(kernels.name());
    }
    if per_core {
        name.push_str("-per-core");
    }
    if pattern == surtr_stress::pattern::Pattern::Transient {
        name.push_str("-transient");
    }
    name
}

/// One stretch of a run: these CPUs, this long. A normal run is one
/// segment; `--per-core` makes one per physical core.
struct Segment {
    cpus: Vec<u32>,
    duration: Duration,
    /// With `--per-core`: the core and its efficiency class.
    core: Option<(u32, Option<u8>)>,
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The segments to run, and the warning to print when running without a
/// temperature sensor.
fn segments(opts: &Options, unwatched: bool) -> Result<(Vec<Segment>, Option<String>), String> {
    let topo = surtr_stress::topology::topology();
    if let Some(each) = opts.per_core {
        if unwatched {
            return Err(
                "--per-core needs a CPU temperature sensor to watch; `surtr caps` shows why it \
                 is missing"
                    .to_owned(),
            );
        }
        let cores = topo.iter().map(|c| c.core).max().map_or(0, |m| m + 1);
        let segments = (0..cores)
            .filter_map(|core| {
                let cpus: Vec<&surtr_stress::topology::LogicalCpu> =
                    topo.iter().filter(|c| c.core == core).collect();
                let first = cpus.first()?;
                Some(Segment {
                    cpus: if opts.siblings {
                        cpus.iter().map(|c| c.id).collect()
                    } else {
                        vec![first.id]
                    },
                    duration: each,
                    core: Some((core, first.efficiency)),
                })
            })
            .collect();
        return Ok((segments, None));
    }
    let cpus: Vec<u32> = match &opts.cpus {
        Some(list) => {
            surtr_stress::topology::check(&topo, list)?;
            list.clone()
        }
        None => surtr_stress::topology::placement(&topo, opts.threads, opts.smt)?,
    };
    let mut duration = opts.duration;
    let mut warning = None;
    if unwatched {
        if !opts.unmonitored {
            return Err(
                "no CPU temperature sensor to watch, so a stress test could overheat this \
                 machine unnoticed; `surtr caps` shows why it is missing. Pass --unmonitored \
                 for a 60-second check without temperature protection."
                    .to_owned(),
            );
        }
        duration = duration.min(runner::UNMONITORED_MAX);
        warning = Some(format!(
            "surtr: warning: no CPU temperature sensor; running {} without temperature protection",
            span(ms(duration))
        ));
    }
    Ok((
        vec![Segment {
            cpus,
            duration,
            core: None,
        }],
        warning,
    ))
}

/// Runs the test; returns the report and the exit code (0 passed, 2 stopped
/// early, 3 errors found or the worker failed).
pub fn run(hub: &Hub, opts: &Options, on_start: impl FnOnce(&str)) -> Result<(Report, u8), String> {
    let watched: Vec<Watched> = runner::watched(&hub.sensors(), opts.max_temp_c);
    let (segments, warning) = segments(opts, watched.is_empty())?;
    let total: Duration = segments.iter().map(|s| s.duration).sum();
    if total > opts.max_duration {
        return Err(format!(
            "{} is longer than the {} limit (stress.max_duration in the settings)",
            span(ms(total)),
            span(ms(opts.max_duration)),
        ));
    }
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let per_core = opts.per_core.is_some();
    // Every CPU the run touches, in the order the view and record show them.
    let mut all_cpus: Vec<u32> = Vec::new();
    for c in segments.iter().flat_map(|s| &s.cpus) {
        if !all_cpus.contains(c) {
            all_cpus.push(*c);
        }
    }
    let position = |cpu: u32| all_cpus.iter().position(|&c| c == cpu).unwrap_or(0);
    let threads = u32::try_from(all_cpus.len()).unwrap_or(u32::MAX);
    let cpu_text = |cpus: &[u32]| {
        cpus.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // A second Ctrl+C after the first falls through to the default.
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let limits: Vec<String> = watched
        .iter()
        .map(|w| format!("{} {:.0} °C", w.label, w.stop_at))
        .collect();
    let limits = if limits.is_empty() {
        "the time limit".to_owned()
    } else {
        limits.join(", ")
    };
    if per_core {
        eprintln!(
            "Testing {} cores one at a time, {} each ({} in all); stops at {limits}. Press \
             Ctrl+C to stop.",
            segments.len(),
            span(ms(segments.first().map_or(Duration::ZERO, |s| s.duration))),
            span(ms(total)),
        );
    } else {
        eprintln!(
            "Stressing {threads} threads on CPUs {} for {}; stops at {limits}. Press Ctrl+C \
             to stop.",
            cpu_text(&all_cpus),
            span(ms(total)),
        );
    }
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
                workload: workload_name(per_core, opts.pattern, opts.kernels),
                state: State::Running,
                started: iso_utc(Timestamp::now()),
                ended: None,
                planned_ms: ms(total),
                threads,
                cpus: all_cpus.clone(),
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
    // The flame view: one flame per CPU, labeled with its number.
    let view_done = Arc::new(AtomicBool::new(false));
    let view = if opts.animate {
        let state = Arc::new(Mutex::new(Live {
            workload: workload_name(per_core, opts.pattern, opts.kernels)
                .trim_start_matches("cpu-")
                .replace('-', " "),
            planned_ms: ms(total),
            limit: watched
                .iter()
                .min_by(|a, b| a.stop_at.total_cmp(&b.stop_at))
                .map(|w| (w.label.clone(), w.stop_at)),
            threads: all_cpus
                .iter()
                .map(|c| ThreadLive {
                    label: c.to_string(),
                    idle: per_core,
                    note: per_core.then(|| "·".to_owned()),
                    ..ThreadLive::default()
                })
                .collect(),
            ..Live::default()
        }));
        // Without a usable terminal, fall back to the one-line status.
        fire::spawn(Arc::clone(&state), Arc::clone(&view_done))
            .ok()
            .map(|handle| (state, handle))
    } else {
        None
    };
    let live_state = view.as_ref().map(|(s, _)| Arc::clone(s));
    if let (Some(j), Some(dir)) = (&journal, runs_dir()) {
        on_start(&j.record.id);
        // An agent may ask a run to stop (`stop_stress_test`) by leaving a
        // file; stopping is always safe.
        let stop = Arc::clone(&stop);
        let file = surtr_stress::request::stop_path(&dir, &j.record.id);
        let live = live_state.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if file.exists() {
                    let _ = std::fs::remove_file(&file);
                    match &live {
                        Some(l) => {
                            l.lock().unwrap_or_else(PoisonError::into_inner).message =
                                Some("stop requested through MCP".to_owned());
                        }
                        None => eprintln!("\nsurtr: stop requested through MCP"),
                    }
                    stop.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
    let line = view.is_none() && std::io::stderr().is_terminal();
    let mut sensors = HubSensors {
        hub,
        watched: watched.iter().map(|w| w.id.clone()).collect(),
        clocks: Vec::new(),
        peaks: Vec::new(),
        view: live_state.clone().map(|s| (s, all_cpus.clone())),
    };

    let mut results: Vec<(&Segment, Report)> = Vec::new();
    let mut offset_ms = 0u64;
    let mut base_iterations = 0u64;
    for (k, seg) in segments.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        sensors.peaks.clear();
        if let Some(state) = &live_state {
            let mut l = state.lock().unwrap_or_else(PoisonError::into_inner);
            for t in &mut l.threads {
                let active = t.label.parse::<u32>().is_ok_and(|c| seg.cpus.contains(&c));
                t.idle = !active;
                if !active {
                    t.rate = None;
                }
            }
            if per_core && l.message.is_none() {
                l.message = Some(format!("core {} of {}", k + 1, segments.len()));
            }
        }
        let mut worker = Command::new(&exe);
        worker.args([
            "worker",
            "cpu",
            "--threads",
            &seg.cpus.len().to_string(),
            "--cpus",
            &cpu_text(&seg.cpus),
            "--duration-ms",
            &seg.duration.as_millis().to_string(),
            "--pattern",
            opts.pattern.name(),
            "--kernels",
            opts.kernels.name(),
        ]);
        // Each segment's thread i is the run's CPU at this position.
        let pos: Vec<usize> = seg.cpus.iter().map(|&c| position(c)).collect();
        let mut last: Option<(u64, Vec<u64>)> = None;
        let report = runner::run(
            worker,
            seg.duration,
            &watched,
            &mut sensors,
            &stop,
            |event, temp| {
                if let (true, Event::Progress { elapsed_ms, .. }) = (line, event) {
                    let shown = temp.map_or_else(String::new, |t| format!(" · {t:.1} °C"));
                    let core = seg
                        .core
                        .map_or_else(String::new, |(c, _)| format!("core {c} · "));
                    eprint!("\r  {core}{}{shown}    ", span(offset_ms + elapsed_ms));
                    let _ = std::io::stderr().flush();
                }
                if let Some(state) = &live_state {
                    let mut l = state.lock().unwrap_or_else(PoisonError::into_inner);
                    l.temp_c = temp.or(l.temp_c);
                    match event {
                        Event::Progress {
                            elapsed_ms,
                            iterations,
                        } => {
                            l.elapsed_ms = offset_ms + elapsed_ms;
                            // Each thread's rate since the last progress event.
                            if let Some((at, before)) = &last {
                                let dt = elapsed_ms.saturating_sub(*at) as f64 / 1000.0;
                                if dt > 0.0 {
                                    for (i, (now, then)) in
                                        iterations.iter().zip(before).enumerate()
                                    {
                                        if let Some(t) =
                                            pos.get(i).and_then(|&p| l.threads.get_mut(p))
                                        {
                                            t.rate = Some(now.saturating_sub(*then) as f64 / dt);
                                        }
                                    }
                                }
                            }
                            last = Some((*elapsed_ms, iterations.clone()));
                        }
                        Event::Error { thread, .. } => {
                            l.errors += 1;
                            if let Some(t) = pos
                                .get(*thread as usize)
                                .and_then(|&p| l.threads.get_mut(p))
                            {
                                t.failed = true;
                            }
                        }
                        Event::Started { .. } | Event::Finished { .. } => {}
                    }
                }
                let Some(j) = journal.as_mut() else { return };
                match event {
                    Event::Progress {
                        elapsed_ms,
                        iterations,
                    } => {
                        j.record.timeline.push(Tick {
                            elapsed_ms: offset_ms + elapsed_ms,
                            temp_c: temp,
                            iterations: base_iterations + iterations.iter().sum::<u64>(),
                        });
                        // Every other second is enough to see what led up to
                        // a reset.
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
        offset_ms += report.duration_ms;
        base_iterations += report.iterations.iter().sum::<u64>();
        let ended_early = matches!(
            report.outcome,
            Outcome::Stopped | Outcome::TemperatureLimit { .. }
        );
        if let Some(state) = &live_state {
            let mut l = state.lock().unwrap_or_else(PoisonError::into_inner);
            for &p in &pos {
                if let Some(t) = l.threads.get_mut(p) {
                    t.idle = per_core;
                    t.rate = if per_core { None } else { t.rate };
                    if per_core && !t.failed && report.outcome == Outcome::Passed {
                        t.note = Some("✓".to_owned());
                    }
                }
            }
        }
        let peak_clock = seg
            .cpus
            .iter()
            .filter_map(|c| sensors.peaks.iter().find(|(p, _)| p == c).map(|(_, v)| *v))
            .reduce(f64::max);
        results.push((seg, report));
        if let Some((_, r)) = results.last_mut()
            && seg.core.is_some()
        {
            // Remembered for the per-core table below.
            r.per_core = vec![CoreResult {
                core: seg.core.map_or(0, |c| c.0),
                cpus: seg.cpus.clone(),
                outcome: r.outcome.clone(),
                duration_ms: r.duration_ms,
                rate: (r.duration_ms > 0).then(|| {
                    r.iterations.iter().sum::<u64>() as f64 / (r.duration_ms as f64 / 1000.0)
                }),
                peak_clock_mhz: peak_clock,
                peak_temp_c: r.peak_temp_c,
                errors: u32::try_from(r.failures.len()).unwrap_or(u32::MAX),
                efficiency: seg.core.and_then(|c| c.1),
            }];
        }
        if ended_early {
            break;
        }
    }

    // One report for the whole run.
    let mut iterations = vec![0u64; all_cpus.len()];
    let mut failures = Vec::new();
    let mut per_core_results = Vec::new();
    let mut peak: Option<f64> = None;
    for (seg, r) in &results {
        for (i, n) in r.iterations.iter().enumerate() {
            if let Some(&c) = seg.cpus.get(i) {
                iterations[position(c)] += n;
            }
        }
        for f in &r.failures {
            let mut f = f.clone();
            if let Some(&c) = seg.cpus.get(f.thread as usize) {
                f.thread = u32::try_from(position(c)).unwrap_or(f.thread);
            }
            failures.push(f);
        }
        per_core_results.extend(r.per_core.iter().cloned());
        peak = match (peak, r.peak_temp_c) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }
    let last_outcome = results.last().map(|(_, r)| r.outcome.clone());
    let outcome = if !failures.is_empty() {
        Outcome::ErrorsFound
    } else if let Some(o @ (Outcome::Stopped | Outcome::TemperatureLimit { .. })) = last_outcome {
        o
    } else if let Some(o @ Outcome::WorkerFailed { .. }) = results
        .iter()
        .map(|(_, r)| r.outcome.clone())
        .find(|o| matches!(o, Outcome::WorkerFailed { .. }))
    {
        o
    } else if results.len() < segments.len() {
        Outcome::Stopped
    } else {
        Outcome::Passed
    };
    let report = Report {
        outcome,
        threads,
        duration_ms: offset_ms,
        iterations,
        failures,
        peak_temp_c: peak,
        watched: watched.clone(),
        // Every segment runs the same computations.
        kernels: results
            .first()
            .map(|(_, r)| r.kernels.clone())
            .unwrap_or_default(),
        per_core: per_core_results,
    };

    if let Some((state, handle)) = view {
        state.lock().unwrap_or_else(PoisonError::into_inner).ending = Some(match report.outcome {
            Outcome::Passed => Ending::Passed,
            Outcome::ErrorsFound => Ending::Failed,
            _ => Ending::Stopped,
        });
        view_done.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }
    // End the one-line status before anything else is printed.
    if line {
        eprintln!();
    }
    if let Some(j) = journal {
        let id = j.record.id.clone();
        j.finish(report.clone(), iso_utc(Timestamp::now()))
            .map_err(|e| format!("could not save the run: {e}"))?;
        eprintln!("Saved as run {id}; `surtr runs report {id}` writes an HTML report.");
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
    if !r.kernels.is_empty() {
        let _ = writeln!(out, "  computations: {}", r.kernels.join(", "));
    }
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
    if !r.per_core.is_empty() {
        let classes: Vec<u8> = r.per_core.iter().filter_map(|c| c.efficiency).collect();
        // Hybrid CPUs: the higher efficiency class is the faster core.
        let hybrid = classes.iter().min() != classes.iter().max();
        let top = classes.iter().max().copied();
        let _ = writeln!(
            out,
            "\n  Core  CPUs     Result       Peak clock  Throughput  Peak temp"
        );
        for c in &r.per_core {
            let result = match &c.outcome {
                Outcome::Passed => "passed".to_owned(),
                Outcome::ErrorsFound => format!("ERRORS ({})", c.errors),
                Outcome::TemperatureLimit { .. } => "temp limit".to_owned(),
                Outcome::Stopped => "stopped".to_owned(),
                Outcome::WorkerFailed { .. } => "FAILED".to_owned(),
            };
            let kind = match (hybrid, c.efficiency) {
                (true, Some(e)) if Some(e) == top => " P",
                (true, Some(_)) => " E",
                _ => "",
            };
            let cpus = c
                .cpus
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(
                out,
                "  {:>4}{kind:<2} {cpus:<8} {result:<12} {:>10}  {:>10}  {:>9}",
                c.core,
                c.peak_clock_mhz
                    .map_or_else(|| "-".to_owned(), |m| format!("{m:.0} MHz")),
                c.rate
                    .map_or_else(|| "-".to_owned(), |r| format!("{r:.0}/s")),
                c.peak_temp_c
                    .map_or_else(|| "-".to_owned(), |t| format!("{t:.1} °C")),
            );
        }
        let bad: Vec<String> = r
            .per_core
            .iter()
            .filter(|c| c.errors > 0 || matches!(c.outcome, Outcome::WorkerFailed { .. }))
            .map(|c| c.core.to_string())
            .collect();
        if !bad.is_empty() {
            let _ = writeln!(
                out,
                "  Unstable core(s): {}. Loosen their per-core setting (for example the Curve \
                 Optimizer offset) and test again.",
                bad.join(", ")
            );
        }
    }
    out
}
