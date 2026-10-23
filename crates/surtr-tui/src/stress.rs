//! The Stress tab: a form for the built-in CPU test, a confirmation, the
//! run with its flames, and the result; past runs and comparing them; and
//! AI agents' requests, which a person approves here or denies. The run itself happens on a thread
//! the event loop starts (see [`crate::run`]); this module only holds the
//! state and turns keys into a request.

use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use surtr_model::Timestamp;
use surtr_stress::{
    Kernels, compare,
    host::{self, Finished, Observer, Options},
    html::{self, span},
    live::Live,
    pattern::Pattern,
    record::{self, Record},
    request::{self, Request, RequestState},
    runner::{UNMONITORED_MAX, Watched},
    topology::{self, LogicalCpu},
};

use crate::fire::Fire;

/// What the Stress tab needs from the command line to start runs.
#[derive(Debug, Clone)]
pub struct StressSetup {
    /// The `surtr` executable, started again as `surtr worker`.
    pub worker: PathBuf,
    /// Where run records live.
    pub runs_dir: Option<PathBuf>,
    /// Temperature limit in °C (`stress.max_temp_c`).
    pub max_temp_c: f64,
    /// Longest allowed run (`stress.max_duration`).
    pub max_duration: Duration,
}

/// Run lengths offered for a whole-CPU run.
pub const LENGTHS: [Duration; 7] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];
/// Time per core offered for a per-core run.
pub const EACH: [Duration; 5] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
];
const KERNELS: [Kernels; 4] = [
    Kernels::Mixed,
    Kernels::Simd,
    Kernels::Cache,
    Kernels::Memory,
];

/// A row of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Which computations.
    Test,
    /// All cores at once, or one at a time.
    Mode,
    /// How many threads (all cores).
    Threads,
    /// Use SMT siblings (all cores), or load both siblings (per core).
    Smt,
    /// Run length, or time per core.
    Length,
    /// Steady or switching on and off.
    Load,
}

/// What the tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Choosing the test.
    Form,
    /// The warning, waiting for `y`.
    Confirm,
    /// Running.
    Running,
    /// Over: the result, or why it didn't start.
    Done,
    /// Past runs.
    Runs,
}

/// The form's values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Index into the test kinds.
    pub test: usize,
    /// One core at a time.
    pub per_core: bool,
    /// Worker threads; `None` is every logical CPU.
    pub threads: Option<u32>,
    /// Use SMT siblings (all cores).
    pub smt: bool,
    /// Load both SMT siblings of each core (per core).
    pub siblings: bool,
    /// Index into [`LENGTHS`].
    pub length: usize,
    /// Index into [`EACH`].
    pub each: usize,
    /// Switching load on and off.
    pub transient: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            test: 0,
            per_core: false,
            threads: None,
            smt: true,
            siblings: false,
            length: 3,
            each: 2,
            transient: false,
        }
    }
}

/// What a running test has told the tab so far; shared with the run thread.
#[derive(Debug, Default)]
pub struct Shared {
    /// The live state, once the load starts.
    pub live: Option<Arc<Mutex<Live>>>,
    /// Lines the host wrote: what runs, warnings.
    pub notes: Vec<String>,
    /// The outcome, once over.
    pub result: Option<Result<Finished, String>>,
}

/// Passes the host's messages to the tab, and marks an agent's request
/// started once its run is recorded.
pub struct Relay {
    shared: Arc<Mutex<Shared>>,
    request: Option<(PathBuf, Request)>,
}

impl Relay {
    fn with(&self, f: impl FnOnce(&mut Shared)) {
        f(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl Observer for Relay {
    fn note(&mut self, text: &str) {
        let text = text.strip_prefix("surtr: ").unwrap_or(text).to_owned();
        self.with(|s| s.notes.push(text));
    }

    fn live(&mut self, state: &Arc<Mutex<Live>>) {
        self.with(|s| s.live = Some(Arc::clone(state)));
    }

    fn recorded(&mut self, id: &str) {
        if let Some((dir, mut req)) = self.request.take() {
            req.state = RequestState::Started;
            req.run = Some(id.to_owned());
            let _ = request::save(&dir, &req);
        }
    }
}

/// A run the event loop should start.
#[derive(Debug)]
pub struct Start {
    /// The options.
    pub options: Options,
    /// Where it reports.
    pub shared: Arc<Mutex<Shared>>,
    /// Stops it.
    pub stop: Arc<AtomicBool>,
    /// The agent's request it answers.
    pub request: Option<Request>,
}

/// The past-runs view.
#[derive(Debug, Default)]
pub struct Runs {
    /// Recorded runs, newest first.
    pub records: Vec<Record>,
    /// The selected row.
    pub row: usize,
    /// The run marked as the baseline for a comparison.
    pub marked: Option<usize>,
    /// A run or a comparison as text, with its title, over the list.
    pub detail: Option<(String, String)>,
}

/// The Stress tab's state.
#[derive(Debug)]
pub struct Stress {
    /// Settings from the command line; `None` where runs can't start.
    pub setup: Option<StressSetup>,
    /// The CPUs, for placement and the per-core count.
    pub topology: Vec<LogicalCpu>,
    /// The temperatures a run would watch, with their stop limits.
    pub watched: Vec<Watched>,
    /// What is shown.
    pub phase: Phase,
    /// The selected form row.
    pub row: usize,
    /// The form.
    pub form: Form,
    /// A run for the event loop to start.
    pub start: Option<Start>,
    /// The current or last run.
    pub shared: Arc<Mutex<Shared>>,
    /// Stops the current run.
    pub stop: Arc<AtomicBool>,
    /// Quit once the run has stopped.
    pub quit_after: bool,
    /// The flames' heat, kept between frames.
    pub fire: RefCell<Fire>,
    /// Agents' requests waiting for a person, oldest first.
    pub pending: Vec<Request>,
    /// The request being reviewed: the confirmation shows it instead of the
    /// form's choices.
    pub request: Option<Request>,
    /// Past runs.
    pub runs: Runs,
    /// The last problem reading or writing runs or requests.
    pub problem: Option<String>,
}

impl Stress {
    /// A tab for this machine.
    pub fn new(
        setup: Option<StressSetup>,
        topology: Vec<LogicalCpu>,
        watched: Vec<Watched>,
    ) -> Self {
        Self {
            setup,
            topology,
            watched,
            phase: Phase::Form,
            row: 0,
            form: Form::default(),
            start: None,
            shared: Arc::default(),
            stop: Arc::default(),
            quit_after: false,
            fire: RefCell::new(Fire::new(0x2545_F491_4F6C_DD1D)),
            pending: Vec::new(),
            request: None,
            runs: Runs::default(),
            problem: None,
        }
    }

    fn runs_dir(&self) -> Option<&PathBuf> {
        self.setup.as_ref()?.runs_dir.as_ref()
    }

    /// Reads the agents' requests again; call it on every refresh.
    pub fn refresh_requests(&mut self) {
        let Some(dir) = self.runs_dir() else { return };
        match request::pending(dir, Timestamp::now().0) {
            Ok(p) => self.pending = p,
            Err(e) => self.problem = Some(format!("Couldn't read the agents' requests: {e}")),
        }
        // One being reviewed that expired or was answered elsewhere is gone.
        if self.phase == Phase::Confirm
            && let Some(r) = &self.request
            && !self.pending.iter().any(|p| p.id == r.id)
        {
            self.request = None;
            self.phase = Phase::Form;
        }
    }

    /// Opens the past runs, newest first.
    pub fn open_runs(&mut self) {
        let Some(dir) = self.runs_dir() else { return };
        match record::load_all(dir) {
            Ok((mut records, _)) => {
                records.reverse();
                self.show_runs(records);
            }
            Err(e) => self.problem = Some(format!("Couldn't read the runs: {e}")),
        }
    }

    /// Shows these runs (newest first).
    pub fn show_runs(&mut self, records: Vec<Record>) {
        self.runs = Runs {
            records,
            ..Runs::default()
        };
        self.phase = Phase::Runs;
    }

    fn deny(&mut self, mut req: Request) {
        let Some(dir) = self.runs_dir().cloned() else {
            return;
        };
        req.state = RequestState::Denied;
        if let Err(e) = request::save(&dir, &req) {
            self.problem = Some(format!("Couldn't turn down request {}: {e}", req.id));
        }
        self.pending.retain(|p| p.id != req.id);
    }

    /// One line about an agent's request.
    pub fn request_text(&self, r: &Request) -> String {
        format!(
            "{} asks to run the {} stress test: {} threads for {}.",
            r.client,
            r.workload,
            r.threads
                .map_or_else(|| format!("all {}", self.logical()), |t| t.to_string()),
            short(Duration::from_millis(r.duration_ms))
        )
    }

    fn runs_key(&mut self, k: KeyEvent) -> bool {
        let runs = &mut self.runs;
        if runs.detail.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Enter => runs.detail = None,
                _ => return false,
            }
            return true;
        }
        let n = runs.records.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => runs.row = runs.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                runs.row = (runs.row + 1).min(n.saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(r) = runs.records.get(runs.row) {
                    runs.detail = Some((r.id.clone(), run_text(r)));
                }
            }
            KeyCode::Char('c') => match runs.marked {
                // The marked run is the baseline; the selected one is compared
                // with it.
                Some(m) if m != runs.row => {
                    if let (Some(a), Some(b)) = (runs.records.get(m), runs.records.get(runs.row)) {
                        let text = compare::text(&compare::compare(a, b));
                        runs.detail = Some((format!("{} → {}", a.id, b.id), text));
                    }
                    runs.marked = None;
                }
                Some(_) => runs.marked = None,
                None if n > 0 => runs.marked = Some(runs.row),
                None => {}
            },
            KeyCode::Esc | KeyCode::Backspace => self.phase = Phase::Form,
            _ => return false,
        }
        true
    }

    /// The form's rows for the chosen mode.
    pub fn fields(&self) -> Vec<Field> {
        if self.form.per_core {
            vec![
                Field::Test,
                Field::Mode,
                Field::Smt,
                Field::Length,
                Field::Load,
            ]
        } else {
            vec![
                Field::Test,
                Field::Mode,
                Field::Threads,
                Field::Smt,
                Field::Length,
                Field::Load,
            ]
        }
    }

    /// The chosen computations.
    pub fn kernels(&self) -> Kernels {
        KERNELS[self.form.test.min(KERNELS.len() - 1)]
    }

    fn logical(&self) -> u32 {
        u32::try_from(self.topology.len())
            .unwrap_or(u32::MAX)
            .max(1)
    }

    fn cores(&self) -> usize {
        self.topology
            .iter()
            .map(|c| c.core)
            .max()
            .map_or(0, |m| m as usize + 1)
    }

    /// A row's label and value, as the form shows them.
    pub fn row_text(&self, field: Field) -> (&'static str, String) {
        let f = &self.form;
        match field {
            Field::Test => (
                "Test",
                match self.kernels() {
                    Kernels::Mixed => "mixed: integer and float".to_owned(),
                    Kernels::Simd => "simd: fused multiply-add, most heat".to_owned(),
                    Kernels::Cache => "cache: FFT in L2".to_owned(),
                    Kernels::Memory => "memory: RAM-sized FFT and pattern test".to_owned(),
                },
            ),
            Field::Mode => (
                "Cores",
                if f.per_core {
                    "one at a time (finds the unstable core)"
                } else {
                    "all at once"
                }
                .to_owned(),
            ),
            Field::Threads => (
                "Threads",
                f.threads
                    .map_or_else(|| format!("all ({})", self.logical()), |t| t.to_string()),
            ),
            Field::Smt => (
                "SMT siblings",
                match (f.per_core, f.smt, f.siblings) {
                    (false, true, _) => "use them",
                    (true, _, true) => "load both siblings",
                    _ => "one thread per core",
                }
                .to_owned(),
            ),
            Field::Length => {
                if f.per_core {
                    ("Per core", short(EACH[f.each]))
                } else {
                    ("Length", short(LENGTHS[f.length]))
                }
            }
            Field::Load => (
                "Load",
                if f.transient {
                    "on and off (transient)"
                } else {
                    "steady"
                }
                .to_owned(),
            ),
        }
    }

    /// The options the form stands for, or the request being reviewed.
    pub fn options(&self) -> Options {
        let setup = self.setup.as_ref();
        let f = &self.form;
        if let Some(r) = &self.request {
            // What `surtr runs confirm` runs: the default test, as asked.
            return Options {
                duration: Duration::from_millis(r.duration_ms),
                max_duration: setup.map_or(Duration::ZERO, |s| s.max_duration),
                max_temp_c: setup.map_or(0.0, |s| s.max_temp_c),
                threads: r.threads,
                cpus: None,
                smt: true,
                per_core: None,
                siblings: false,
                pattern: Pattern::Steady,
                kernels: Kernels::Mixed,
                unmonitored: self.watched.is_empty(),
                requested_by: Some(r.client.clone()),
            };
        }
        Options {
            duration: LENGTHS[f.length],
            max_duration: setup.map_or(Duration::ZERO, |s| s.max_duration),
            max_temp_c: setup.map_or(0.0, |s| s.max_temp_c),
            threads: f.threads,
            cpus: None,
            smt: f.smt,
            per_core: f.per_core.then(|| EACH[f.each]),
            siblings: f.siblings,
            pattern: if f.transient {
                Pattern::Transient
            } else {
                Pattern::Steady
            },
            kernels: self.kernels(),
            // Shown and confirmed before it starts.
            unmonitored: self.watched.is_empty(),
            requested_by: None,
        }
    }

    /// What the run would do, in a line or two; an `Err` says why it can't
    /// start.
    pub fn plan(&self) -> Result<Vec<String>, String> {
        let Some(setup) = &self.setup else {
            return Err("Stress tests aren't available here.".to_owned());
        };
        let o = self.options();
        let mut lines = Vec::new();
        let total = if let Some(each) = o.per_core {
            if self.watched.is_empty() {
                return Err(
                    "Testing one core at a time needs a CPU temperature sensor to \
                     watch; the Capabilities view shows why it is missing."
                        .to_owned(),
                );
            }
            let cores = self.cores();
            let total = each * u32::try_from(cores).unwrap_or(u32::MAX);
            lines.push(format!(
                "{cores} cores one at a time, {} each: {} in all.",
                short(each),
                short(total)
            ));
            total
        } else {
            let cpus = topology::placement(&self.topology, o.threads, o.smt)?;
            let mut length = o.duration;
            if self.watched.is_empty() {
                length = length.min(UNMONITORED_MAX);
            }
            lines.push(format!(
                "{} threads on CPUs {} for {}.",
                cpus.len(),
                cpus.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                short(length)
            ));
            length
        };
        if total > setup.max_duration {
            return Err(format!(
                "{} is longer than the {} limit (stress.max_duration in the settings).",
                short(total),
                short(setup.max_duration)
            ));
        }
        if self.watched.is_empty() {
            lines.push(format!(
                "No CPU temperature sensor: it runs at most {} without temperature protection.",
                short(UNMONITORED_MAX)
            ));
        } else {
            let limits: Vec<String> = self
                .watched
                .iter()
                .map(|w| format!("{} {:.0} °C", w.label, w.stop_at))
                .collect();
            lines.push(format!("Stops at {}.", limits.join(", ")));
        }
        Ok(lines)
    }

    /// Whether a run is going.
    pub fn running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// The live state of the current or last run.
    pub fn live(&self) -> Option<Live> {
        let shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let live = shared.live.as_ref()?;
        Some(live.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    /// Picks up a finished run; returns whether the app should quit now.
    pub fn poll(&mut self) -> bool {
        if self.phase == Phase::Running
            && self
                .shared
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .result
                .is_some()
        {
            self.phase = Phase::Done;
            return self.quit_after;
        }
        false
    }

    /// Stops the run, if one is going.
    pub fn stop_run(&mut self) {
        if self.running() {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn change(&mut self, field: Field, forward: bool) {
        let step = |i: usize, n: usize| {
            if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }
        };
        let logical = self.logical();
        let f = &mut self.form;
        match field {
            Field::Test => f.test = step(f.test, KERNELS.len()),
            Field::Mode => f.per_core = !f.per_core,
            Field::Threads => {
                // all, 1, 2, ..., logical, all
                f.threads = match (f.threads, forward) {
                    (None, true) => Some(1),
                    (None, false) => Some(logical),
                    (Some(t), true) if t >= logical => None,
                    (Some(t), true) => Some(t + 1),
                    (Some(t), false) if t <= 1 => None,
                    (Some(t), false) => Some(t - 1),
                };
            }
            Field::Smt if f.per_core => f.siblings = !f.siblings,
            Field::Smt => f.smt = !f.smt,
            Field::Length => {
                if f.per_core {
                    f.each = step(f.each, EACH.len());
                } else {
                    f.length = step(f.length, LENGTHS.len());
                }
            }
            Field::Load => f.transient = !f.transient,
        }
    }

    /// Handles a key; returns whether it was used (otherwise it goes to the
    /// app: tabs, quit, help).
    pub fn key(&mut self, k: KeyEvent) -> bool {
        if self.setup.is_none() {
            return false;
        }
        match self.phase {
            Phase::Runs => self.runs_key(k),
            Phase::Form => {
                let fields = self.fields();
                self.row = self.row.min(fields.len() - 1);
                match k.code {
                    KeyCode::Up | KeyCode::Char('k') => self.row = self.row.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.row = (self.row + 1).min(fields.len() - 1);
                    }
                    KeyCode::Left | KeyCode::Char('h') => self.change(fields[self.row], false),
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
                        self.change(fields[self.row], true);
                    }
                    KeyCode::Enter => {
                        self.request = None;
                        if self.plan().is_ok() {
                            self.phase = Phase::Confirm;
                        }
                    }
                    KeyCode::Char('r') => self.open_runs(),
                    // The oldest request waiting: review it, or turn it down.
                    KeyCode::Char('a') if !self.pending.is_empty() => {
                        self.request = self.pending.first().cloned();
                        self.phase = Phase::Confirm;
                    }
                    KeyCode::Char('d') if !self.pending.is_empty() => {
                        let req = self.pending[0].clone();
                        self.deny(req);
                    }
                    _ => return false,
                }
                true
            }
            Phase::Confirm => {
                match k.code {
                    KeyCode::Char('y' | 'Y') => {
                        self.shared = Arc::default();
                        self.stop = Arc::default();
                        if self.plan().is_err() {
                            return true;
                        }
                        self.start = Some(Start {
                            options: self.options(),
                            shared: Arc::clone(&self.shared),
                            stop: Arc::clone(&self.stop),
                            request: self.request.take(),
                        });
                        *self.fire.borrow_mut() = Fire::new(0x2545_F491_4F6C_DD1D);
                        self.phase = Phase::Running;
                    }
                    // n turns an agent's request down, as in `surtr runs confirm`;
                    // Esc leaves it waiting.
                    KeyCode::Char('n' | 'N') => {
                        if let Some(req) = self.request.take() {
                            self.deny(req);
                        }
                        self.phase = Phase::Form;
                    }
                    KeyCode::Esc | KeyCode::Backspace => {
                        self.request = None;
                        self.phase = Phase::Form;
                    }
                    _ => {}
                }
                // Nothing else while the question is open.
                k.code != KeyCode::Char('q')
            }
            Phase::Running => match k.code {
                KeyCode::Char('s') => {
                    self.stop_run();
                    true
                }
                _ => false,
            },
            Phase::Done => match k.code {
                KeyCode::Enter => {
                    self.phase = Phase::Form;
                    true
                }
                _ => false,
            },
        }
    }
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Relay")
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A recorded run as text, like `surtr runs show`.
pub fn run_text(r: &Record) -> String {
    let mut out = format!(
        "{}
",
        html::verdict(r).0
    );
    match &r.report {
        Some(rep) => out.push_str(&host::describe(rep)),
        None => {
            let s = compare::summary(r);
            out.push_str(&format!(
                "  {} threads · {} recorded{}
",
                r.threads,
                span(s.length_ms),
                s.peak_temp_c
                    .map_or_else(String::new, |p| format!(" · peak {p:.1} °C"))
            ));
        }
    }
    if let Some(by) = &r.requested_by {
        out.push_str(&format!(
            "  asked for by {by}
"
        ));
    }
    out
}

/// A length for the form: `5m` for whole minutes, else as runs show it.
fn short(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 && s.is_multiple_of(60) && d.subsec_millis() == 0 {
        format!("{}m", s / 60)
    } else {
        span(ms(d))
    }
}

/// Runs `start` on this thread with the hub; for the event loop's thread.
pub fn run(hub: &surtr_core::Hub, setup: &StressSetup, start: Start) {
    let host_setup = host::Setup {
        worker: setup.worker.clone(),
        runs_dir: setup.runs_dir.clone(),
        stop: start.stop,
    };
    let mut relay = Relay {
        shared: Arc::clone(&start.shared),
        request: start
            .request
            .and_then(|r| Some((setup.runs_dir.clone()?, r))),
    };
    let result = host::run(hub, &start.options, &host_setup, &mut relay);
    start
        .shared
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .result = Some(result);
}

#[cfg(test)]
pub(crate) mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn press(s: &mut Stress, code: KeyCode) -> bool {
        s.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Four cores with two threads each, numbered in sibling pairs.
    pub fn topo() -> Vec<LogicalCpu> {
        (0..8)
            .map(|id| LogicalCpu {
                id,
                core: id / 2,
                efficiency: None,
            })
            .collect()
    }

    pub fn setup() -> StressSetup {
        StressSetup {
            worker: PathBuf::from("surtr"),
            runs_dir: None,
            max_temp_c: 95.0,
            max_duration: Duration::from_secs(30 * 60),
        }
    }

    pub fn tctl() -> Vec<Watched> {
        vec![Watched {
            id: "cpu/temp".to_owned(),
            label: "Tctl".to_owned(),
            stop_at: 95.0,
        }]
    }

    #[test]
    fn form_builds_the_options() {
        let mut s = Stress::new(Some(setup()), topo(), tctl());
        assert_eq!(
            s.plan().unwrap(),
            [
                "8 threads on CPUs 0,2,4,6,1,3,5,7 for 5m.",
                "Stops at Tctl 95 °C."
            ]
        );
        // Test: simd. Threads: 2, no SMT.
        press(&mut s, KeyCode::Right);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Right);
        press(&mut s, KeyCode::Right);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Right);
        let o = s.options();
        assert_eq!(
            (o.kernels, o.threads, o.smt, o.per_core),
            (Kernels::Simd, Some(2), false, None)
        );
        assert_eq!(s.plan().unwrap()[0], "2 threads on CPUs 0,2 for 5m.");
        // Mode: per core, both siblings; the threads row goes away.
        s.row = 1;
        press(&mut s, KeyCode::Right);
        assert_eq!(s.fields().len(), 5);
        s.row = 2;
        press(&mut s, KeyCode::Right);
        let o = s.options();
        assert_eq!((o.per_core, o.siblings), (Some(EACH[2]), true));
        assert_eq!(
            s.plan().unwrap()[0],
            "4 cores one at a time, 1m each: 4m in all."
        );
    }

    #[test]
    fn confirm_then_start_then_stop() {
        let mut s = Stress::new(Some(setup()), topo(), tctl());
        assert!(press(&mut s, KeyCode::Enter));
        assert_eq!(s.phase, Phase::Confirm);
        press(&mut s, KeyCode::Esc);
        assert_eq!(s.phase, Phase::Form);
        press(&mut s, KeyCode::Enter);
        // Anything but y leaves it waiting.
        press(&mut s, KeyCode::Enter);
        assert!(s.start.is_none());
        press(&mut s, KeyCode::Char('y'));
        assert_eq!(s.phase, Phase::Running);
        let start = s.start.take().unwrap();
        assert_eq!(start.options.duration, LENGTHS[3]);
        assert!(press(&mut s, KeyCode::Char('s')));
        assert!(start.stop.load(Ordering::Relaxed));
        // The result arrives: the tab shows it.
        start.shared.lock().unwrap().result = Some(Err("gone".to_owned()));
        assert!(!s.poll());
        assert_eq!(s.phase, Phase::Done);
        press(&mut s, KeyCode::Enter);
        assert_eq!(s.phase, Phase::Form);
    }

    #[test]
    fn limits_and_missing_sensors_block_or_warn() {
        let mut s = Stress::new(Some(setup()), topo(), Vec::new());
        // Without a sensor: a short unmonitored run, and no per-core runs.
        let plan = s.plan().unwrap();
        assert_eq!(plan[0], "8 threads on CPUs 0,2,4,6,1,3,5,7 for 1m.");
        assert!(plan[1].starts_with("No CPU temperature sensor"));
        assert!(s.options().unmonitored);
        s.form.per_core = true;
        assert!(s.plan().is_err());
        press(&mut s, KeyCode::Enter);
        assert_eq!(s.phase, Phase::Form);
        // Longer than stress.max_duration.
        let mut s = Stress::new(Some(setup()), topo(), tctl());
        s.setup.as_mut().unwrap().max_duration = Duration::from_secs(60);
        assert!(s.plan().unwrap_err().contains("longer than the 1m limit"));
        // Not set up at all.
        let s = Stress::new(None, topo(), tctl());
        assert!(s.plan().is_err());
    }

    #[test]
    fn threads_cycle_through_all() {
        let mut s = Stress::new(Some(setup()), topo(), tctl());
        s.row = 2;
        press(&mut s, KeyCode::Left);
        assert_eq!(s.form.threads, Some(8));
        press(&mut s, KeyCode::Right);
        assert_eq!(s.form.threads, None);
        press(&mut s, KeyCode::Right);
        assert_eq!(s.form.threads, Some(1));
    }

    /// A finished 60-second run at `rate` iterations per second.
    pub fn record(id: &str, rate: u64, peak: f64) -> Record {
        use surtr_stress::{
            record::{SCHEMA, State, Tick},
            runner::{Outcome, Report},
        };
        let timeline: Vec<Tick> = (0..=60u64)
            .map(|s| Tick {
                elapsed_ms: s * 1000,
                temp_c: Some(peak - 5.0 + s as f64 / 12.0),
                iterations: s * rate,
            })
            .collect();
        Record {
            schema: SCHEMA,
            id: id.into(),
            workload: "cpu".into(),
            state: State::Done,
            started: "2026-10-23T14:12:00Z".into(),
            ended: None,
            planned_ms: 60_000,
            threads: 2,
            cpus: vec![0, 2],
            surtr: "0.3.0".into(),
            machine: Vec::new(),
            watched: Vec::new(),
            requested_by: None,
            errors_seen: 0,
            timeline,
            report: Some(Report {
                outcome: Outcome::Passed,
                threads: 2,
                duration_ms: 60_000,
                iterations: vec![30 * rate, 30 * rate],
                failures: Vec::new(),
                peak_temp_c: Some(peak),
                watched: Vec::new(),
                kernels: vec!["integer".into(), "float".into()],
                per_core: Vec::new(),
            }),
        }
    }

    /// A fresh runs directory for one test.
    fn runs_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("surtr-tui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn past_runs_show_and_compare() {
        let dir = runs_dir("runs");
        for (id, rate) in [("20261023-100000-cpu", 1000), ("20261023-110000-cpu", 1100)] {
            let r = record(id, rate, 70.0);
            let report = r.report.clone().unwrap();
            record::Journal::start(&dir, Record { report: None, ..r })
                .unwrap()
                .finish(report, "2026-10-23T14:13:00Z".into())
                .unwrap();
        }
        let mut setup = setup();
        setup.runs_dir = Some(dir.clone());
        let mut s = Stress::new(Some(setup), topo(), tctl());
        press(&mut s, KeyCode::Char('r'));
        assert_eq!(s.phase, Phase::Runs);
        // Newest first.
        assert_eq!(s.runs.records[0].id, "20261023-110000-cpu");
        press(&mut s, KeyCode::Enter);
        let (title, text) = s.runs.detail.clone().unwrap();
        assert_eq!(title, "20261023-110000-cpu");
        assert!(text.contains("PASSED"), "{text}");
        press(&mut s, KeyCode::Esc);
        assert!(s.runs.detail.is_none());
        // Mark the older run as the baseline, then compare the newer one.
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Char('c'));
        press(&mut s, KeyCode::Up);
        press(&mut s, KeyCode::Char('c'));
        let (title, text) = s.runs.detail.clone().unwrap();
        assert_eq!(title, "20261023-100000-cpu → 20261023-110000-cpu");
        assert!(text.contains("+10.0%"), "{text}");
        press(&mut s, KeyCode::Esc);
        press(&mut s, KeyCode::Esc);
        assert_eq!(s.phase, Phase::Form);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_requests_are_approved_or_denied_here() {
        let dir = runs_dir("requests");
        let now = Timestamp::now().0;
        let first = request::create(&dir, "agent 1.0", "cpu", 120_000, Some(2), now).unwrap();
        let mut setup = setup();
        setup.runs_dir = Some(dir.clone());
        let mut s = Stress::new(Some(setup), topo(), tctl());
        s.refresh_requests();
        assert_eq!(s.pending.len(), 1);
        assert_eq!(
            s.request_text(&s.pending[0]),
            "agent 1.0 asks to run the cpu stress test: 2 threads for 2m."
        );
        // Review it: the confirmation shows the request, not the form.
        press(&mut s, KeyCode::Char('a'));
        assert_eq!(s.phase, Phase::Confirm);
        assert_eq!(s.plan().unwrap()[0], "2 threads on CPUs 0,2 for 2m.");
        assert_eq!(s.options().requested_by.as_deref(), Some("agent 1.0"));
        // Esc leaves it waiting; n turns it down.
        press(&mut s, KeyCode::Esc);
        assert_eq!(s.pending.len(), 1);
        press(&mut s, KeyCode::Char('a'));
        press(&mut s, KeyCode::Char('n'));
        let state = |id: &str| request::load(&dir, id, now).unwrap().unwrap().state;
        assert_eq!(state(&first.id), RequestState::Denied);
        assert!(s.pending.is_empty());
        // d turns one down from the form.
        let second = request::create(&dir, "agent 1.0", "cpu", 60_000, None, now).unwrap();
        s.refresh_requests();
        press(&mut s, KeyCode::Char('d'));
        assert_eq!(state(&second.id), RequestState::Denied);
        // y starts it; once recorded, the request points at its run.
        let third = request::create(&dir, "agent 1.0", "cpu", 60_000, None, now).unwrap();
        s.refresh_requests();
        press(&mut s, KeyCode::Char('a'));
        press(&mut s, KeyCode::Char('y'));
        let start = s.start.take().unwrap();
        assert_eq!(start.options.requested_by.as_deref(), Some("agent 1.0"));
        let mut relay = Relay {
            shared: start.shared,
            request: Some((dir.clone(), start.request.unwrap())),
        };
        relay.recorded("20261023-120000-cpu");
        let got = request::load(&dir, &third.id, now).unwrap().unwrap();
        assert_eq!(
            (got.state, got.run.as_deref()),
            (RequestState::Started, Some("20261023-120000-cpu"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
