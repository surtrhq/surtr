//! The live view of a stress run: one flame per CPU thread, in the
//! terminal, below the command line (an inline viewport, so scrollback stays
//! intact).
//!
//! The art carries the data: a flame's height is that thread's throughput
//! (throttling makes the flames drop), its color is the temperature against
//! the stop limit (orange and yellow when cool, deep red near the limit), a
//! thread that gave a wrong answer burns down to grey ash with a mark under
//! it, and a passing run dies down to embers at the end. The flames are the
//! classic "Doom fire": heat rises and cools cell by cell, drawn with
//! half-blocks for twice the vertical resolution.

use std::{
    io,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
};

use surtr_stress::live::{Ending, Live, ThreadLive};

use crate::colors::Depth;

/// Rows the view takes.
pub const HEIGHT: u16 = 12;
/// Hottest cell.
const MAX_HEAT: u8 = 36;
/// Time between frames.
const FRAME: Duration = Duration::from_millis(80);

/// A column of the fire: one thread, or several on a narrow terminal.
#[derive(Debug, Clone, PartialEq)]
struct Column {
    x: u16,
    width: u16,
    label: String,
    rate: Option<f64>,
    clock_mhz: Option<f64>,
    failed: bool,
    idle: bool,
    note: Option<String>,
}

/// Lays threads out across `width` cells: one column each with a one-cell
/// gap, or threads grouped when there are too many to be at least two
/// cells wide.
fn columns(threads: &[ThreadLive], width: u16) -> Vec<Column> {
    if threads.is_empty() || width < 3 {
        return Vec::new();
    }
    let n = threads.len();
    let per = n.div_ceil(usize::from(width) / 3).max(1);
    let groups: Vec<&[ThreadLive]> = threads.chunks(per).collect();
    let g = u16::try_from(groups.len()).unwrap_or(u16::MAX);
    let cw = ((width + 1) / g).saturating_sub(1).clamp(2, 10);
    let used = g * (cw + 1) - 1;
    let left = width.saturating_sub(used) / 2;
    groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let rates: Vec<f64> = group.iter().filter_map(|t| t.rate).collect();
            let clocks: Vec<f64> = group.iter().filter_map(|t| t.clock_mhz).collect();
            let label = match (group.first(), group.last()) {
                (Some(a), Some(b)) if group.len() > 1 => format!("{}-{}", a.label, b.label),
                (Some(a), _) => a.label.clone(),
                _ => String::new(),
            };
            Column {
                x: left + u16::try_from(i).unwrap_or(0) * (cw + 1),
                width: cw,
                label,
                rate: (!rates.is_empty()).then(|| rates.iter().sum::<f64>() / rates.len() as f64),
                clock_mhz: (!clocks.is_empty())
                    .then(|| clocks.iter().sum::<f64>() / clocks.len() as f64),
                failed: group.iter().any(|t| t.failed),
                idle: group.iter().all(|t| t.idle),
                note: group.iter().find_map(|t| t.note.clone()),
            }
        })
        .collect()
}

/// The fire's heat grid and its random numbers.
#[derive(Debug, Clone)]
pub struct Fire {
    width: usize,
    height: usize,
    heat: Vec<u8>,
    rng: u64,
    /// The highest rate seen, slowly forgotten, so heights compare threads
    /// with each other and with themselves a moment ago.
    peak_rate: f64,
    /// Frames since the run ended.
    after: u32,
}

impl Fire {
    /// An unlit fire; `seed` makes the flicker repeatable (tests).
    pub fn new(seed: u64) -> Self {
        Self {
            width: 0,
            height: 0,
            heat: Vec::new(),
            rng: seed | 1,
            peak_rate: 0.0,
            after: 0,
        }
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// One frame of fire over a grid `width` cells wide and `height` half-rows
    /// high, fed per cell column by `fuel` (0 to 1; `None` for a gap).
    fn step(&mut self, width: usize, height: usize, fuel: &[Option<f64>]) {
        if width != self.width || height != self.height {
            self.width = width;
            self.height = height;
            self.heat = vec![0; width * height];
        }
        if width == 0 || height < 2 {
            return;
        }
        let bottom = (height - 1) * width;
        for x in 0..width {
            let f = fuel.get(x).copied().flatten().unwrap_or(0.0);
            let jitter = 0.8 + (self.random() % 21) as f64 / 100.0;
            self.heat[bottom + x] = (f * f64::from(MAX_HEAT) * jitter).round() as u8;
        }
        let lit = |x: usize| fuel.get(x).copied().flatten().is_some();
        for y in 0..height - 1 {
            for x in 0..width {
                // Gaps between flames stay dark.
                let Some(f) = fuel.get(x).copied().flatten() else {
                    self.heat[y * width + x] = 0;
                    continue;
                };
                // Each cell pulls heat from below, wavering left or right but
                // never from outside its own flame. Cooling per step sets the
                // height: about f of the grid.
                let cool =
                    ((2.0 * f64::from(MAX_HEAT)) / (f.max(0.05) * height as f64)).clamp(1.0, 18.0);
                let r = self.random();
                let decay = (r % (cool.round() as u64 + 1)) as u8;
                let from = match (r >> 8) % 3 {
                    0 if x > 0 && lit(x - 1) => x - 1,
                    2 if x + 1 < width && lit(x + 1) => x + 1,
                    _ => x,
                };
                let src = self.heat[(y + 1) * width + from];
                self.heat[y * width + x] = src.saturating_sub(decay);
            }
        }
    }

    fn at(&self, x: usize, y: usize) -> u8 {
        self.heat.get(y * self.width + x).copied().unwrap_or(0)
    }
}

/// Blends `a` and `b`, `t` of the way to `b`.
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let m = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The color along `stops`, `t` from 0 to 1.
fn gradient(stops: &[(u8, u8, u8)], t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (t.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], t - i as f64)
}

/// Flame colors when cool: embers to yellow-white.
const COOL: [(u8, u8, u8); 6] = [
    (0x3a, 0x0d, 0x00),
    (0x8a, 0x1e, 0x00),
    (0xd9, 0x4a, 0x00),
    (0xff, 0x7a, 0x1a),
    (0xff, 0xb3, 0x3a),
    (0xff, 0xe8, 0xa0),
];
/// Flame colors near the limit: deep and angry reds.
const HOT: [(u8, u8, u8); 6] = [
    (0x30, 0x00, 0x00),
    (0x70, 0x00, 0x08),
    (0xb0, 0x10, 0x10),
    (0xe0, 0x28, 0x18),
    (0xff, 0x50, 0x30),
    (0xff, 0x90, 0x70),
];
/// A thread that gave a wrong answer.
const ASH: [(u8, u8, u8); 3] = [(0x26, 0x26, 0x26), (0x55, 0x55, 0x55), (0x8b, 0x93, 0x9e)];

/// The color of a cell with `heat`, `hot` (0 to 1) of the way to the limit.
fn flame(heat: u8, hot: f64, ash: bool, depth: Depth) -> Option<Color> {
    if heat == 0 {
        return None;
    }
    let t = f64::from(heat) / f64::from(MAX_HEAT);
    match depth {
        Depth::TrueColor => {
            let c = if ash {
                gradient(&ASH, t)
            } else {
                mix(gradient(&COOL, t), gradient(&HOT, t), hot)
            };
            Some(Color::Rgb(c.0, c.1, c.2))
        }
        Depth::Basic => Some(match (ash, hot > 0.6, (t * 4.0) as u8) {
            (true, _, 0..=1) => Color::DarkGray,
            (true, _, _) => Color::Gray,
            (false, false, 0) | (false, true, 0..=1) => Color::Red,
            (false, false, 1) | (false, true, 2) => Color::LightRed,
            (false, false, 2) | (false, true, _) => Color::Yellow,
            (false, false, _) => Color::LightYellow,
        }),
    }
}

/// `1m 05s` for milliseconds.
fn span(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        3600.. => format!("{}h {:02}m", s / 3600, s / 60 % 60),
        60.. => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{s}s"),
    }
}

fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, area: Rect) {
    if y < area.bottom() && x < area.right() {
        buf.set_stringn(x, y, s, usize::from(area.right() - x), style);
    }
}

/// Draws `live` into `area`, advancing `fire` one frame.
pub fn draw(frame: &mut Frame, area: Rect, live: &Live, fire: &mut Fire, depth: Depth) {
    render(frame.buffer_mut(), area, live, fire, depth);
}

/// [`draw`] on a buffer.
pub fn render(buf: &mut Buffer, area: Rect, live: &Live, fire: &mut Fire, depth: Depth) {
    if area.height < 7 || area.width < 20 {
        return;
    }
    let rgb = |c: (u8, u8, u8), basic: Color| match depth {
        Depth::TrueColor => Color::Rgb(c.0, c.1, c.2),
        Depth::Basic => basic,
    };
    let accent = Style::new()
        .fg(rgb((0xff, 0x7a, 0x1a), Color::Yellow))
        .add_modifier(Modifier::BOLD);
    let muted = Style::new().add_modifier(Modifier::DIM);
    let hot = match (live.temp_c, &live.limit) {
        (Some(t), Some((_, l))) => ((t - (l - 25.0)) / 25.0).clamp(0.0, 1.0),
        _ => 0.0,
    };

    // Header: what runs, and the temperature against its limit.
    let title = format!(
        "SURTR · {} stress · {} / {}",
        live.workload,
        span(live.elapsed_ms),
        span(live.planned_ms)
    );
    text(buf, area.x + 1, area.y, &title, accent, area);
    if let Some(t) = live.temp_c {
        let limit = live.limit.as_ref().map_or_else(String::new, |(label, l)| {
            format!("  {label} stop at {l:.0} °C")
        });
        let temp = format!("{t:.1} °C{limit}");
        let style = Style::new()
            .fg(rgb(
                gradient(&[(0xf2, 0xc1, 0x4e), (0xff, 0x5c, 0x5c)], hot),
                if hot > 0.6 { Color::Red } else { Color::Yellow },
            ))
            .add_modifier(Modifier::BOLD);
        let x = area
            .right()
            .saturating_sub(u16::try_from(temp.chars().count()).unwrap_or(0) + 1);
        text(buf, x.max(area.x), area.y, &temp, style, area);
    }

    // The fire, between the header and four rows: floor, CPUs, clocks and
    // status.
    let rows = area.height - 5;
    let fire_top = area.y + 1;
    let width = area.width - 2;
    let cols = columns(&live.threads, width);
    fire.peak_rate =
        (fire.peak_rate * 0.995).max(cols.iter().filter_map(|c| c.rate).fold(0.0, f64::max));
    if live.ending.is_some() {
        fire.after += 1;
    }
    let dying = match live.ending {
        Some(Ending::Passed) => (1.0 - f64::from(fire.after) / 15.0).max(0.12),
        Some(Ending::Stopped) => (1.0 - f64::from(fire.after) / 15.0).max(0.0),
        _ => 1.0,
    };
    let mut fuel: Vec<Option<f64>> = vec![None; usize::from(width)];
    let mut ash = vec![false; usize::from(width)];
    for c in &cols {
        let f = if c.failed {
            0.25
        } else if c.idle {
            0.04
        } else {
            match c.rate {
                Some(r) if fire.peak_rate > 0.0 => (r / fire.peak_rate).clamp(0.05, 1.0),
                _ => 0.35,
            }
        };
        for x in c.x..c.x + c.width {
            fuel[usize::from(x)] = Some(f * dying);
            ash[usize::from(x)] = c.failed;
        }
    }
    let h = usize::from(rows) * 2;
    fire.step(usize::from(width), h, &fuel);
    for row in 0..rows {
        for x in 0..width {
            let (gx, top, bottom) = (
                usize::from(x),
                usize::from(row) * 2,
                usize::from(row) * 2 + 1,
            );
            let up = flame(fire.at(gx, top), hot, ash[gx], depth);
            let down = flame(fire.at(gx, bottom), hot, ash[gx], depth);
            let cell = &mut buf[(area.x + 1 + x, fire_top + row)];
            match (up, down) {
                (None, None) => {
                    cell.set_char(' ');
                }
                (Some(u), None) => {
                    cell.set_char('▀').set_fg(u);
                }
                (None, Some(d)) => {
                    cell.set_char('▄').set_fg(d);
                }
                (Some(u), Some(d)) => {
                    cell.set_char('▀').set_fg(u).set_bg(d);
                }
            }
        }
    }

    // A floor under the fire, then each column's CPU and clock.
    let floor = fire_top + rows;
    text(
        buf,
        area.x + 1,
        floor,
        &"─".repeat(usize::from(width)),
        muted,
        area,
    );
    for c in &cols {
        let fit = |s: &str| -> String {
            let w = usize::from(c.width);
            let s: String = s.chars().take(w).collect();
            format!("{s:^w$}")
        };
        text(
            buf,
            area.x + 1 + c.x,
            floor + 1,
            &fit(&c.label),
            muted,
            area,
        );
        let (below, style) = if c.failed {
            (
                "✗".to_owned(),
                Style::new()
                    .fg(rgb((0xff, 0x5c, 0x5c), Color::Red))
                    .add_modifier(Modifier::BOLD),
            )
        } else if let Some(note) = c.note.as_ref().filter(|_| c.idle) {
            (note.clone(), muted)
        } else {
            (
                c.clock_mhz
                    .map_or_else(String::new, |m| format!("{:.1}G", m / 1000.0)),
                Style::new(),
            )
        };
        text(buf, area.x + 1 + c.x, floor + 2, &fit(&below), style, area);
    }

    // Progress and errors.
    let status_y = area.bottom() - 1;
    let errors = match live.errors {
        0 => "0 errors".to_owned(),
        1 => "1 ERROR".to_owned(),
        n => format!("{n} ERRORS"),
    };
    let err_style = if live.errors > 0 {
        Style::new()
            .fg(rgb((0xff, 0x5c, 0x5c), Color::Red))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(rgb((0x3f, 0xb9, 0x50), Color::Green))
    };
    let tail = format!(
        " {:>3}%  {errors}{}",
        (live.elapsed_ms * 100)
            .checked_div(live.planned_ms)
            .unwrap_or(0)
            .min(100),
        live.message
            .as_ref()
            .map_or_else(String::new, |m| format!("  · {m}"))
    );
    let bar_w = width
        .saturating_sub(u16::try_from(tail.chars().count()).unwrap_or(0) + 1)
        .max(4);
    let done = (u64::from(bar_w) * live.elapsed_ms)
        .checked_div(live.planned_ms)
        .unwrap_or(0)
        .min(u64::from(bar_w));
    let done = u16::try_from(done).unwrap_or(bar_w);
    text(
        buf,
        area.x + 1,
        status_y,
        &"█".repeat(usize::from(done)),
        accent,
        area,
    );
    text(
        buf,
        area.x + 1 + done,
        status_y,
        &"░".repeat(usize::from(bar_w - done)),
        muted,
        area,
    );
    let tail_x = area.x + 1 + bar_w;
    let pct_end = tail.find("  ").map_or(tail.len(), |i| i + 2);
    text(buf, tail_x, status_y, &tail[..pct_end], Style::new(), area);
    text(
        buf,
        tail_x + u16::try_from(tail[..pct_end].chars().count()).unwrap_or(0),
        status_y,
        &tail[pct_end..],
        err_style,
        area,
    );
}

/// Runs the view on its own thread, drawing `state` about 12 times a second
/// into a [`HEIGHT`]-row viewport on standard error, until `done` is set; then
/// it plays the ending for a moment and leaves the last frame on screen.
pub fn spawn(state: Arc<Mutex<Live>>, done: Arc<AtomicBool>) -> io::Result<thread::JoinHandle<()>> {
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(HEIGHT),
        },
    )?;
    let depth = Depth::detect();
    Ok(thread::spawn(move || {
        let mut fire = Fire::new(0x2545_F491_4F6C_DD1D);
        let mut ending_frames = 0;
        loop {
            let live = state.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let _ = terminal.draw(|f| draw(f, f.area(), &live, &mut fire, depth));
            if done.load(Ordering::Relaxed) || live.ending.is_some() {
                ending_frames += 1;
                if ending_frames > 18 {
                    break;
                }
            }
            thread::sleep(FRAME);
        }
        // Leave the last frame and continue below it.
        let bottom = terminal.get_frame().area().bottom();
        let _ = terminal.set_cursor_position(Position::new(0, bottom.saturating_sub(1)));
        let _ = terminal.show_cursor();
        eprintln!();
    }))
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn threads(n: usize) -> Vec<ThreadLive> {
        (0..n)
            .map(|i| ThreadLive {
                label: i.to_string(),
                rate: Some(400.0 - i as f64 * 20.0),
                clock_mhz: Some(3500.0 - i as f64 * 50.0),
                failed: i == 5,
                ..ThreadLive::default()
            })
            .collect()
    }

    fn live() -> Live {
        Live {
            workload: "cpu".into(),
            elapsed_ms: 134_000,
            planned_ms: 300_000,
            temp_c: Some(78.4),
            limit: Some(("Tctl".into(), 95.0)),
            threads: threads(8),
            errors: 1,
            message: None,
            ending: None,
        }
    }

    #[test]
    fn columns_fit_the_width() {
        let cols = columns(&threads(8), 60);
        assert_eq!(cols.len(), 8);
        assert!(cols.iter().all(|c| c.width >= 2));
        let last = cols.last().unwrap();
        assert!(last.x + last.width <= 60);
        // Too many threads for the width: grouped, never narrower than 2.
        let cols = columns(&threads(64), 40);
        assert!(cols.len() <= 40 / 3);
        assert!(cols.iter().all(|c| c.width >= 2));
        assert!(cols[0].label.contains('-'), "{:?}", cols[0].label);
        assert!(columns(&[], 40).is_empty());
    }

    #[test]
    fn colors_follow_heat_temperature_and_ash() {
        assert_eq!(flame(0, 0.0, false, Depth::TrueColor), None);
        let Some(Color::Rgb(r, g, _)) = flame(MAX_HEAT, 0.0, false, Depth::TrueColor) else {
            panic!()
        };
        assert!(r == 0xff && g > 0xd0, "cool tips are yellow-white");
        let Some(Color::Rgb(r, g, _)) = flame(MAX_HEAT, 1.0, false, Depth::TrueColor) else {
            panic!()
        };
        assert!(r == 0xff && g < 0xa0, "near the limit the tips go red");
        let Some(Color::Rgb(r, g, b)) = flame(MAX_HEAT / 2, 0.0, true, Depth::TrueColor) else {
            panic!()
        };
        assert!(r == g && g.abs_diff(b) < 0x20, "ash is grey");
        assert_eq!(
            flame(MAX_HEAT, 1.0, false, Depth::Basic),
            Some(Color::Yellow)
        );
    }

    #[test]
    fn flames_track_fuel() {
        let mut fire = Fire::new(7);
        let fuel = [Some(1.0), Some(1.0), None, Some(0.2), Some(0.2)];
        for _ in 0..60 {
            fire.step(5, 40, &fuel);
        }
        let lit = |x: usize| (0..40).filter(|&y| fire.at(x, y) > 0).count();
        assert!(
            lit(0) > lit(3) + 8,
            "full fuel burns higher: {} vs {}",
            lit(0),
            lit(3)
        );
    }

    #[test]
    fn frame_snapshot() {
        let mut terminal = Terminal::new(TestBackend::new(72, HEIGHT)).unwrap();
        let mut fire = Fire::new(42);
        for _ in 0..30 {
            terminal
                .draw(|f| draw(f, f.area(), &live(), &mut fire, Depth::Basic))
                .unwrap();
        }
        let screen = terminal.backend().to_string();
        assert!(screen.contains("SURTR · cpu stress · 2m 14s / 5m 00s"));
        assert!(screen.contains("78.4 °C  Tctl stop at 95 °C"));
        assert!(screen.contains("1 ERROR"));
        assert!(screen.contains('✗'), "the failed CPU is marked");
        assert!(screen.contains("3.5G"));
        assert!(
            screen.contains('▀') || screen.contains('▄'),
            "there is fire"
        );
        insta::assert_snapshot!("stress_fire_72x12", screen);
    }

    #[test]
    fn small_areas_do_not_panic() {
        let mut fire = Fire::new(1);
        for (w, h) in [(1, 1), (19, 12), (40, 5), (20, 6), (300, 40)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            render(
                &mut buf,
                Rect::new(0, 0, w, h),
                &live(),
                &mut fire,
                Depth::TrueColor,
            );
        }
    }
}
