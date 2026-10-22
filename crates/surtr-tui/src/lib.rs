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
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
};
use surtr_core::Hub;
use surtr_view::Theme;

mod app;
mod colors;
mod ui;

pub use app::{App, Tab};
pub use colors::{Depth, Palette};
pub use ui::draw;

/// Runs the TUI on the current terminal until the user quits, refreshing
/// every `interval`. `name` resolves process ids to names.
pub fn run(
    hub: &Hub,
    interval: Duration,
    theme: Theme,
    name: &dyn Fn(u32) -> Option<String>,
) -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "the TUI needs an interactive terminal; use `surtr sensors` or `surtr status` for piped output",
        ));
    }
    let palette = Palette::new(theme, Depth::detect());
    let mut app = App::new(hub);
    // ratatui::init also restores the terminal if Surtr panics.
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let result = (|| -> io::Result<()> {
        let mut next = Instant::now();
        while !app.quit {
            if Instant::now() >= next {
                app.refresh(hub, name);
                next = Instant::now() + interval;
            }
            terminal.draw(|f| draw(f, &app, &palette))?;
            let wait = next.saturating_duration_since(Instant::now());
            if event::poll(wait)? {
                app.handle(&event::read()?);
            }
        }
        Ok(())
    })();
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
        assert_eq!(app.palette_matches().len(), 4);
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
        assert_eq!(app.tab, Tab::Overview);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 1);
        press(&mut app, KeyCode::Left);
        assert_eq!((app.tab, app.scroll), (Tab::Capabilities, 0));
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
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
