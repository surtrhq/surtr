//! TUI state: the latest readings, a short history per sensor for trends,
//! and what the user is looking at.

use std::collections::{HashMap, VecDeque};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use surtr_core::{Hub, Sample};
use surtr_model::{Capabilities, GpuProcess, SystemInfo, ThrottleState};
use surtr_view::Status;

/// How many samples each trend keeps (two minutes at the default 1 s).
pub const HISTORY: usize = 120;

/// The views, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Health summary, system and findings.
    Overview,
    /// Every sensor with its trend.
    Sensors,
    /// Per-GPU gauges, throttle reasons and processes.
    Gpu,
    /// What works on this machine.
    Capabilities,
}

impl Tab {
    /// All tabs in order.
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Sensors, Tab::Gpu, Tab::Capabilities];

    /// Tab title.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Sensors => "Sensors",
            Tab::Gpu => "GPU",
            Tab::Capabilities => "Capabilities",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }
}

/// Everything the TUI shows.
#[derive(Debug)]
pub struct App {
    /// Current view.
    pub tab: Tab,
    /// First row shown in scrollable views.
    pub scroll: usize,
    /// Whether the help overlay is open.
    pub help: bool,
    /// Set when the user asks to quit.
    pub quit: bool,
    /// Provider ids, for the footer.
    pub providers: Vec<String>,
    /// Hardware snapshot (read once).
    pub system: SystemInfo,
    /// Capabilities (read once).
    pub capabilities: Capabilities,
    /// Latest sensor values.
    pub samples: Vec<Sample>,
    /// Recent values per `(provider, sensor id)`, oldest first.
    pub history: HashMap<(String, String), VecDeque<f64>>,
    /// Latest health summary.
    pub status: Status,
    /// Latest throttle reasons.
    pub throttle: Vec<ThrottleState>,
    /// Latest GPU processes.
    pub processes: Vec<GpuProcess>,
}

fn key(s: &Sample) -> (String, String) {
    (
        s.descriptor.source.provider.clone(),
        s.descriptor.id.0.clone(),
    )
}

impl App {
    /// Reads the static parts once and primes rate sensors; call
    /// [`App::refresh`] for values.
    pub fn new(hub: &Hub) -> Self {
        let system = hub.system_info();
        let capabilities = hub.capabilities();
        let samples = hub.read();
        let status = surtr_view::status(&system, &samples, &[], &capabilities);
        Self {
            tab: Tab::Overview,
            scroll: 0,
            help: false,
            quit: false,
            providers: hub.provider_ids().into_iter().map(str::to_owned).collect(),
            system,
            capabilities,
            samples,
            history: HashMap::new(),
            status,
            throttle: Vec::new(),
            processes: Vec::new(),
        }
    }

    /// Reads everything once more; `name` fills in process names.
    pub fn refresh(&mut self, hub: &Hub, name: &dyn Fn(u32) -> Option<String>) {
        self.samples = hub.read();
        for s in &self.samples {
            if let Some(v) = s.value {
                let trend = self.history.entry(key(s)).or_default();
                if trend.len() == HISTORY {
                    trend.pop_front();
                }
                trend.push_back(v);
            }
        }
        self.throttle = hub.throttle_reasons();
        self.processes = hub.gpu_processes();
        for p in self.processes.iter_mut().filter(|p| p.name.is_none()) {
            p.name = name(p.pid);
        }
        self.status = surtr_view::status(
            &self.system,
            &self.samples,
            &self.throttle,
            &self.capabilities,
        );
    }

    /// Recent values of a sample's sensor.
    pub fn trend(&self, s: &Sample) -> Option<&VecDeque<f64>> {
        self.history.get(&key(s))
    }

    fn select(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0;
        }
    }

    fn step_tab(&mut self, forward: bool) {
        let n = Tab::ALL.len();
        let i = self.tab.index();
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.select(Tab::ALL[next]);
    }

    /// Applies a terminal event.
    pub fn handle(&mut self, event: &Event) {
        match event {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.key(*k),
            Event::Mouse(m) => self.mouse(*m),
            _ => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if self.help {
            // Any key closes help; q still quits.
            self.help = false;
            if k.code != KeyCode::Char('q') {
                return;
            }
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Tab | KeyCode::Right => self.step_tab(true),
            KeyCode::BackTab | KeyCode::Left => self.step_tab(false),
            KeyCode::Char(c @ '1'..='4') => {
                self.select(Tab::ALL[usize::from(c as u8 - b'1')]);
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll += 10,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = 0,
            _ => {}
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollDown => self.scroll += 3,
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            // A click on the tab bar (top row) selects a tab by its position.
            MouseEventKind::Down(_) if m.row == 0 => {
                if let Some(tab) = crate::ui::tab_at(m.column) {
                    self.select(tab);
                }
            }
            _ => {}
        }
    }
}
