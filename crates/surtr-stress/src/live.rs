//! What a running stress test looks like right now: the state the live views
//! draw (the flame view under the command line, the TUI's Stress tab). The
//! host updates it as the worker's events arrive.

/// One worker thread, as the view shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadLive {
    /// Column label, for example `3` for CPU 3.
    pub label: String,
    /// Iterations per second lately.
    pub rate: Option<f64>,
    /// Clock of its CPU, in MHz.
    pub clock_mhz: Option<f64>,
    /// It gave a wrong answer.
    pub failed: bool,
    /// Not loaded right now (waiting for its turn, or done): low embers.
    pub idle: bool,
    /// Shown under the flame in place of the clock, for example a check mark.
    pub note: Option<String>,
}

/// How the run ended, for the last frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// No errors: the flames die down to embers.
    Passed,
    /// Wrong answers: the failed threads stay ash, the rest stays lit.
    Failed,
    /// Stopped early (limit, user, agent).
    Stopped,
}

/// Everything the view shows; the host updates it as events arrive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// Workload, for example `cpu`.
    pub workload: String,
    /// Milliseconds since the start.
    pub elapsed_ms: u64,
    /// Planned length in milliseconds.
    pub planned_ms: u64,
    /// Highest watched temperature.
    pub temp_c: Option<f64>,
    /// Where the run stops, and the sensor's label.
    pub limit: Option<(String, f64)>,
    /// One entry per worker thread.
    pub threads: Vec<ThreadLive>,
    /// Wrong answers so far.
    pub errors: u32,
    /// A one-line note, such as a stop request.
    pub message: Option<String>,
    /// Set when the run is over.
    pub ending: Option<Ending>,
}
