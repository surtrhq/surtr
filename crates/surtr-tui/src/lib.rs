//! Surtr's terminal UI (`surtr tui`): Overview, Sensors, GPU and Capabilities
//! views for desktops, headless boxes and SSH sessions (plan, section 7.2).
//!
//! [`App`] holds state, `ui` draws it, and [`run`] drives a real terminal.
//! Drawing is pure, so tests render into ratatui's in-memory backend.

#![forbid(unsafe_code)]

use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

use ratatui::crossterm::{
    event::{self, DisableMouseCapture},
    execute,
};
use surtr_core::Hub;
use surtr_view::Theme;

mod app;
mod colors;
pub mod fire;
mod keys;
pub mod stress;
mod ui;

pub use app::{App, Tab};
pub use colors::{Depth, Palette};
pub use stress::StressSetup;
pub use ui::draw;

/// Mouse reports for clicks and the wheel only (modes 1000 and 1006). The
/// usual capture also reports every movement, a flood of escape sequences
/// that over SSH can arrive split and be misread as keys; the TUI doesn't
/// use movement.
#[cfg(unix)]
fn mouse_on() -> io::Result<()> {
    use std::io::Write as _;
    let mut out = io::stdout();
    out.write_all(b"\x1b[?1000h\x1b[?1006h")?;
    out.flush()
}

/// On Windows crossterm reads the mouse through the console API: no escape
/// sequences to split.
#[cfg(not(unix))]
fn mouse_on() -> io::Result<()> {
    execute!(io::stdout(), event::EnableMouseCapture)
}

/// How often the flames are redrawn while a stress test is on screen.
const FRAME: Duration = Duration::from_millis(80);

/// Runs the TUI on the current terminal until the user quits, refreshing
/// every `interval`. `name` resolves process ids to names; with `stress`,
/// the Stress tab can start runs.
pub fn run(
    hub: &Hub,
    interval: Duration,
    theme: Theme,
    name: &dyn Fn(u32) -> Option<String>,
    stress: Option<StressSetup>,
) -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "the TUI needs an interactive terminal; use `surtr sensors` or `surtr status` for piped output",
        ));
    }
    let palette = Palette::new(theme, Depth::detect());
    let mut app = App::new(hub);
    if let Some(setup) = &stress {
        app.stress = stress::Stress::new(
            Some(setup.clone()),
            surtr_stress::topology::topology(),
            surtr_stress::runner::watched(&hub.sensors(), setup.max_temp_c),
        );
    }
    // ratatui::init also restores the terminal if Surtr panics.
    let mut terminal = ratatui::init();
    mouse_on()?;
    // Runs borrow the hub, so they live in this scope; leaving it waits for
    // a stopped run to finish.
    let result = std::thread::scope(|scope| {
        let result = (|| -> io::Result<()> {
            let mut next = Instant::now();
            while !app.quit {
                if Instant::now() >= next {
                    app.refresh(hub, name);
                    next = Instant::now() + interval;
                }
                if let (Some(start), Some(setup)) = (app.stress.start.take(), &stress) {
                    scope.spawn(move || stress::run(hub, setup, start));
                }
                app.tick();
                terminal.draw(|f| draw(f, &app, &palette))?;
                let mut wait = next.saturating_duration_since(Instant::now());
                if app.tab == Tab::Stress && app.stress.live().is_some() {
                    wait = wait.min(FRAME);
                }
                if event::poll(wait)? {
                    let next = |t| -> io::Result<_> {
                        Ok(if event::poll(t)? {
                            Some(event::read()?)
                        } else {
                            None
                        })
                    };
                    for e in keys::reassemble(event::read()?, next)? {
                        app.handle(&e);
                    }
                }
            }
            Ok(())
        })();
        // Leaving with a run going (a terminal error): stop it.
        app.stress
            .stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        result
    });
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        Terminal,
        backend::TestBackend,
        crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers},
    };
    use surtr_backend::mock::MockProvider;

    fn mock_app() -> (Hub, App) {
        let mut hub = Hub::new();
        hub.add(Box::new(MockProvider::desktop()));
        let mut app = App::new(&hub);
        // A few refreshes so trends have values.
        for _ in 0..30 {
            app.refresh(&hub, &|_| None);
        }
        (hub, app)
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let palette = Palette::new(Theme::DARK, Depth::Basic);
        terminal.draw(|f| draw(f, app, &palette)).unwrap();
        terminal.backend().to_string()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    // Every view at a small and a large terminal size (issue #13).
    #[test]
    fn view_snapshots() {
        let (_hub, mut app) = mock_app();
        for tab in Tab::ALL {
            app.tab = tab;
            for (w, h) in [(80, 24), (120, 40)] {
                let name = format!("{}_{w}x{h}", tab.title().to_lowercase());
                insta::assert_snapshot!(name, render(&app, w, h));
            }
        }
    }

    #[test]
    fn help_overlay_snapshot() {
        let (_hub, mut app) = mock_app();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        insta::assert_snapshot!("help_80x24", render(&app, 80, 24));
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.help && !app.quit);
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn palette_filters_and_runs_commands() {
        let (_hub, mut app) = mock_app();
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "go to");
        assert_eq!(app.palette_matches().len(), 5);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        insta::assert_snapshot!("palette_80x24", render(&app, 80, 24));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, Tab::Gpu);
        assert!(app.palette.is_none());
        // Typing doesn't trigger shortcuts while the palette is open.
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "q");
        assert!(!app.quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.palette.is_none() && !app.quit);
    }

    #[test]
    fn slash_filters_sensors() {
        let (_hub, mut app) = mock_app();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.tab, Tab::Sensors);
        type_text(&mut app, "temp");
        insta::assert_snapshot!("sensors_filter_80x24", render(&app, 80, 24));
        press(&mut app, KeyCode::Enter);
        assert!(!app.filtering);
        assert_eq!(app.filter, "temp");
        let shown = app.samples.iter().filter(|s| app.shown(s)).count();
        assert_eq!(shown, 2);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Esc);
        assert!(app.filter.is_empty());
    }

    #[test]
    fn keys_switch_views_and_quit() {
        let (_hub, mut app) = mock_app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.tab, Tab::Gpu);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tab, Tab::Capabilities);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tab, Tab::Stress);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tab, Tab::Overview);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 1);
        press(&mut app, KeyCode::Left);
        assert_eq!((app.tab, app.scroll), (Tab::Stress, 0));
        // Esc never quits: over SSH a split arrow key starts with a lone Esc.
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn stress_tab_form_and_confirm() {
        use crate::stress::{Phase, Stress, tests};
        let (_hub, mut app) = mock_app();
        app.stress = Stress::new(Some(tests::setup()), tests::topo(), tests::tctl());
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.tab, Tab::Stress);
        // Arrows work the form here; Tab still switches views.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!((app.tab, app.stress.form.threads), (Tab::Stress, Some(8)));
        insta::assert_snapshot!("stress_form_80x24", render(&app, 80, 24));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.stress.phase, Phase::Confirm);
        insta::assert_snapshot!("stress_confirm_80x24", render(&app, 80, 24));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.stress.running() && app.stress.start.is_some());
        // Quitting stops the run first, then quits once it has ended.
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit && app.stress.stop.load(std::sync::atomic::Ordering::Relaxed));
        app.stress.shared.lock().unwrap().result = Some(Err("stopped".to_owned()));
        app.tick();
        assert!(app.quit);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn stress_tab_past_runs() {
        use crate::stress::{Stress, tests};
        let (_hub, mut app) = mock_app();
        app.stress = Stress::new(Some(tests::setup()), tests::topo(), tests::tctl());
        app.tab = Tab::Stress;
        app.stress.show_runs(vec![
            tests::record("20261023-110000-cpu", 1100, 71.0),
            tests::record("20261023-100000-cpu", 1000, 70.0),
        ]);
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Down);
        insta::assert_snapshot!("stress_runs_100x24", render(&app, 100, 24));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let (_hub, mut app) = mock_app();
        for tab in Tab::ALL {
            app.tab = tab;
            render(&app, 20, 5);
            render(&app, 1, 1);
        }
    }
}
