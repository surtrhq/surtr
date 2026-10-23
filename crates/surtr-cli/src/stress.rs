//! `surtr stress cpu`: the built-in CPU test on the command line (plan,
//! section 5.3 and M6). The run itself is `surtr_stress::host`; this module
//! wires it to the terminal: Ctrl+C, the flame view or a one-line status,
//! and the report as text.

use std::{
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
    runner::Report,
};
use surtr_tui::fire;

pub use surtr_stress::host::{Options, describe};

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
