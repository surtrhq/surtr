//! Terminal styling for theme roles.
//!
//! Ordinary text keeps the terminal's own colors; headings are bold,
//! secondary text is dimmed, and status roles use the basic ANSI color closest
//! in hue to the theme color, so the terminal's palette (dark or light) decides
//! the exact shade. `anstream` strips all of it when output is piped or
//! `NO_COLOR` is set.

use anstyle::{AnsiColor, Color, Style};
use surtr_view::{Role, Theme};

fn ansi(index: u8) -> AnsiColor {
    match index {
        1 => AnsiColor::Red,
        2 => AnsiColor::Green,
        3 => AnsiColor::Yellow,
        4 => AnsiColor::Blue,
        5 => AnsiColor::Magenta,
        6 => AnsiColor::Cyan,
        7 => AnsiColor::White,
        _ => AnsiColor::Black,
    }
}

fn style(role: Role) -> Style {
    let color = || Some(Color::Ansi(ansi(Theme::DARK.color(role).basic())));
    match role {
        Role::Text => Style::new(),
        Role::Heading => Style::new().bold(),
        Role::Muted | Role::Missing => Style::new().dimmed(),
        Role::Accent | Role::Ok | Role::Warn => Style::new().fg_color(color()),
        Role::Critical => Style::new().fg_color(color()).bold(),
    }
}

/// `text` styled for `role`. Pad before painting: escape codes don't count
/// toward the width of `format!` padding.
pub fn paint(role: Role, text: &str) -> String {
    let style = style(role);
    if style == Style::new() {
        text.to_owned()
    } else {
        format!("{style}{text}{style:#}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_codes() {
        assert_eq!(paint(Role::Text, "x"), "x");
        assert_ne!(paint(Role::Critical, "x"), "x");
        assert_eq!(
            anstream::adapter::strip_str(&paint(Role::Warn, "hot")).to_string(),
            "hot"
        );
    }

    #[test]
    fn status_roles_use_matching_hues() {
        assert_eq!(
            style(Role::Ok).get_fg_color(),
            Some(Color::Ansi(AnsiColor::Green))
        );
        assert_eq!(
            style(Role::Warn).get_fg_color(),
            Some(Color::Ansi(AnsiColor::Yellow))
        );
        assert_eq!(
            style(Role::Critical).get_fg_color(),
            Some(Color::Ansi(AnsiColor::Red))
        );
    }
}
