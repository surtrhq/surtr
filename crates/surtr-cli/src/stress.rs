//! `surtr stress cpu`: the built-in CPU test on the command line (plan,
//! section 5.3 and M6). The run itself is `surtr_stress::host`; this module
//! wires it to the terminal: Ctrl+C, the flame view or a one-line status,
//! and the report as text.

use std::{
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use surtr_core::{Hub, paths};
use surtr_stress::{
    host::{self, Observer},
    html::span,
    live::Live,
    runner::{Outcome, Report},
};
use surtr_tui::fire;

pub use surtr_stress::host::Options;

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

/// Shows a run on standard error.
struct Console<F> {
    /// Show the flame view when the terminal allows it.
    animate: bool,
    /// Told the run's id once it is recorded.
    on_start: Option<F>,
    /// The flame view's thread, while it runs.
    view: Option<JoinHandle<()>>,
    view_done: Arc<AtomicBool>,
    /// The one-line status, when there is no flame view.
    line: bool,
}

impl<F: FnOnce(&str)> Observer for Console<F> {
    fn note(&mut self, text: &str) {
        eprintln!("{text}");
    }

    fn banner(&mut self, text: &str) {
        eprintln!("{text} Press Ctrl+C to stop.");
    }

    fn live(&mut self, state: &Arc<Mutex<Live>>) {
        if self.animate {
            // Without a usable terminal, fall back to the one-line status.
            self.view = fire::spawn(Arc::clone(state), Arc::clone(&self.view_done)).ok();
        }
        self.line = self.view.is_none() && std::io::stderr().is_terminal();
    }

    fn recorded(&mut self, id: &str) {
        if let Some(f) = self.on_start.take() {
            f(id);
        }
    }

    fn progress(&mut self, elapsed_ms: u64, core: Option<u32>, temp_c: Option<f64>) {
        if self.line {
            let shown = temp_c.map_or_else(String::new, |t| format!(" · {t:.1} °C"));
            let core = core.map_or_else(String::new, |c| format!("core {c} · "));
            eprint!("\r  {core}{}{shown}    ", span(elapsed_ms));
            let _ = std::io::stderr().flush();
        }
    }

    fn stop_requested(&mut self) {
        // The flame view shows it as its message.
        if self.view.is_none() {
            eprintln!("\nsurtr: stop requested through MCP");
        }
    }

    fn ended(&mut self) {
        if let Some(handle) = self.view.take() {
            self.view_done.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
        // End the one-line status before anything else is printed.
        if self.line {
            eprintln!();
        }
    }
}

/// Runs the test; returns the report and the exit code (0 passed, 2 stopped
/// early, 3 errors found or the worker failed).
pub fn run(
    hub: &Hub,
    opts: &Options,
    animate: bool,
    on_start: impl FnOnce(&str),
) -> Result<(Report, u8), String> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // A second Ctrl+C after the first falls through to the default.
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }
    let setup = host::Setup {
        worker: std::env::current_exe().map_err(|e| e.to_string())?,
        runs_dir: runs_dir(),
        stop,
    };
    let mut console = Console {
        animate,
        on_start: Some(on_start),
        view: None,
        view_done: Arc::new(AtomicBool::new(false)),
        line: false,
    };
    let done = host::run(hub, opts, &setup, &mut console)?;
    if let Some(id) = &done.id {
        eprintln!("Saved as run {id}; `surtr runs report {id}` writes an HTML report.");
    }
    let code = host::exit_code(&done.report.outcome);
    Ok((done.report, code))
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
