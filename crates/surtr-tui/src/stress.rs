//! The Stress tab: a form for the built-in CPU test, a confirmation, the
//! run with its flames, and the result. The run itself happens on a thread
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
use surtr_stress::{
    Kernels,
    host::{self, Finished, Observer, Options},
    html::span,
    live::Live,
    pattern::Pattern,
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

/// Passes the host's messages to the tab.
pub struct Relay(pub Arc<Mutex<Shared>>);

impl Relay {
    fn with(&self, f: impl FnOnce(&mut Shared)) {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
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
        }
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

    /// The options the form stands for.
    pub fn options(&self) -> Options {
        let setup = self.setup.as_ref();
        let f = &self.form;
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
        let f = &self.form;
        let mut lines = Vec::new();
        let total = if f.per_core {
            if self.watched.is_empty() {
                return Err(
                    "Testing one core at a time needs a CPU temperature sensor to \
                     watch; the Capabilities view shows why it is missing."
                        .to_owned(),
                );
            }
            let cores = self.cores();
            let each = EACH[f.each];
            let total = each * u32::try_from(cores).unwrap_or(u32::MAX);
            lines.push(format!(
                "{cores} cores one at a time, {} each: {} in all.",
                short(each),
                short(total)
            ));
            total
        } else {
            let cpus = topology::placement(&self.topology, f.threads, f.smt)?;
            let mut length = LENGTHS[f.length];
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
                        if self.plan().is_ok() {
                            self.phase = Phase::Confirm;
                        }
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
                        self.start = Some(Start {
                            options: self.options(),
                            shared: Arc::clone(&self.shared),
                            stop: Arc::clone(&self.stop),
                        });
                        *self.fire.borrow_mut() = Fire::new(0x2545_F491_4F6C_DD1D);
                        self.phase = Phase::Running;
                    }
                    KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Backspace => {
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
    let mut relay = Relay(Arc::clone(&start.shared));
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
}
