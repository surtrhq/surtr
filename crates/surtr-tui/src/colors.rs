//! Theme roles as terminal styles. Ordinary text keeps the terminal's colors
//! and background, so the TUI fits dark and light terminals; accents and
//! status colors come from the theme, in truecolor when the terminal says it
//! supports it (`COLORTERM`), otherwise as the basic color closest in hue.

use ratatui::style::{Color, Modifier, Style};
use surtr_view::{Role, Theme};

/// Color depth to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 24-bit color.
    TrueColor,
    /// The 8 basic colors.
    Basic,
}

impl Depth {
    /// What this terminal supports, from `COLORTERM`.
    pub fn detect() -> Self {
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor" | "24bit") => Self::TrueColor,
            _ => Self::Basic,
        }
    }
}

/// Maps roles to styles.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    theme: Theme,
    depth: Depth,
}

impl Palette {
    /// A palette for `theme` at `depth`.
    pub fn new(theme: Theme, depth: Depth) -> Self {
        Self { theme, depth }
    }

    /// The color of `role`.
    pub fn color(&self, role: Role) -> Color {
        let c = self.theme.color(role);
        match self.depth {
            Depth::TrueColor => Color::Rgb(c.0, c.1, c.2),
            Depth::Basic => match c.basic() {
                1 => Color::Red,
                2 => Color::Green,
                3 => Color::Yellow,
                4 => Color::Blue,
                5 => Color::Magenta,
                6 => Color::Cyan,
                7 => Color::Gray,
                _ => Color::DarkGray,
            },
        }
    }

    /// The style of `role`.
    pub fn style(&self, role: Role) -> Style {
        match role {
            Role::Text => Style::new(),
            Role::Heading => Style::new().add_modifier(Modifier::BOLD),
            Role::Muted | Role::Missing => Style::new().add_modifier(Modifier::DIM),
            Role::Accent | Role::Ok | Role::Warn => Style::new().fg(self.color(role)),
            Role::Critical => Style::new()
                .fg(self.color(role))
                .add_modifier(Modifier::BOLD),
        }
    }
}
