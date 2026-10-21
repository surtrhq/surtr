//! Design tokens: one color per [`Role`], defined once for every front end.
//! The GUI uses them as pixels; terminals use them as truecolor and fall back
//! to the nearest basic color.

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// The closest of the 8 basic terminal colors by hue, for terminals
    /// without truecolor. Returns the ANSI index: 0 black, 1 red, 2 green,
    /// 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 white.
    pub fn basic(self) -> u8 {
        let (r, g, b) = (f64::from(self.0), f64::from(self.1), f64::from(self.2));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        // Grays (low saturation) become black or white by lightness.
        if max == 0.0 || (max - min) / max < 0.25 {
            return if max >= 128.0 { 7 } else { 0 };
        }
        let d = max - min;
        let hue = if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        match hue {
            h if !(20.0..330.0).contains(&h) => 1,
            h if h < 75.0 => 3,
            h if h < 165.0 => 2,
            h if h < 200.0 => 6,
            h if h < 270.0 => 4,
            _ => 5,
        }
    }
}

/// What a piece of text means, independent of its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Ordinary text and values.
    Text,
    /// Secondary text: labels, units, hints.
    Muted,
    /// Section titles.
    Heading,
    /// Interactive or highlighted elements; the single accent color.
    Accent,
    /// Healthy or available.
    Ok,
    /// Needs attention.
    Warn,
    /// Problem.
    Critical,
    /// A value that isn't available.
    Missing,
}

/// A set of colors for every role, plus the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Name, for settings.
    pub name: &'static str,
    /// Background.
    pub background: Rgb,
    /// [`Role::Text`].
    pub text: Rgb,
    /// [`Role::Muted`].
    pub muted: Rgb,
    /// [`Role::Heading`].
    pub heading: Rgb,
    /// [`Role::Accent`].
    pub accent: Rgb,
    /// [`Role::Ok`].
    pub ok: Rgb,
    /// [`Role::Warn`].
    pub warn: Rgb,
    /// [`Role::Critical`].
    pub critical: Rgb,
    /// [`Role::Missing`].
    pub missing: Rgb,
}

impl Theme {
    /// The default: dark, one warm accent (the fire giant's ember).
    pub const DARK: Theme = Theme {
        name: "dark",
        background: Rgb(0x14, 0x16, 0x1a),
        text: Rgb(0xe6, 0xe8, 0xeb),
        muted: Rgb(0x8b, 0x93, 0x9e),
        heading: Rgb(0xf5, 0xf6, 0xf7),
        accent: Rgb(0xff, 0x7a, 0x1a),
        ok: Rgb(0x4c, 0xc3, 0x8a),
        warn: Rgb(0xf2, 0xc1, 0x4e),
        critical: Rgb(0xff, 0x5c, 0x5c),
        missing: Rgb(0x6b, 0x72, 0x7c),
    };

    /// Light variant with the same roles.
    pub const LIGHT: Theme = Theme {
        name: "light",
        background: Rgb(0xfa, 0xfa, 0xf9),
        text: Rgb(0x1f, 0x23, 0x28),
        muted: Rgb(0x5f, 0x67, 0x72),
        heading: Rgb(0x0f, 0x12, 0x16),
        accent: Rgb(0xd9, 0x5a, 0x00),
        ok: Rgb(0x1a, 0x7f, 0x4f),
        warn: Rgb(0x9a, 0x6a, 0x00),
        critical: Rgb(0xc4, 0x2b, 0x2b),
        missing: Rgb(0x8a, 0x91, 0x99),
    };

    /// The color of `role`.
    pub fn color(&self, role: Role) -> Rgb {
        match role {
            Role::Text => self.text,
            Role::Muted => self.muted,
            Role::Heading => self.heading,
            Role::Accent => self.accent,
            Role::Ok => self.ok,
            Role::Warn => self.warn,
            Role::Critical => self.critical,
            Role::Missing => self.missing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: [Role; 8] = [
        Role::Text,
        Role::Muted,
        Role::Heading,
        Role::Accent,
        Role::Ok,
        Role::Warn,
        Role::Critical,
        Role::Missing,
    ];

    /// WCAG relative luminance.
    fn luminance(Rgb(r, g, b): Rgb) -> f64 {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    // Every role must be readable on the background (WCAG AA for large text,
    // 3:1; body text roles 4.5:1).
    #[test]
    fn roles_are_readable() {
        for theme in [Theme::DARK, Theme::LIGHT] {
            for role in ROLES {
                let needed = match role {
                    Role::Text | Role::Heading | Role::Muted => 4.5,
                    _ => 3.0,
                };
                let c = contrast(theme.color(role), theme.background);
                assert!(
                    c >= needed,
                    "{} {role:?}: contrast {c:.2} < {needed}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn basic_colors() {
        assert_eq!(Rgb(0, 0, 0).basic(), 0);
        assert_eq!(Rgb(0xff, 0x5c, 0x5c).basic(), 1); // red
        assert_eq!(Rgb(0x4c, 0xc3, 0x8a).basic(), 2); // green
        assert_eq!(Rgb(0xf2, 0xc1, 0x4e).basic(), 3); // yellow
        assert_eq!(Rgb(0xe6, 0xe8, 0xeb).basic(), 7); // white
    }
}
