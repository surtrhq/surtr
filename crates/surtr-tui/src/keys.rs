//! Escape sequences that arrived in pieces. Over SSH the bytes of a key or
//! mouse sequence (`ESC [ A`, `ESC [ < 64;10;5 M`) can be split across
//! packets; crossterm then reports a lone Esc followed by the rest as
//! ordinary characters, which used to quit the TUI or switch tabs (the
//! digits) while moving the mouse. After a lone Esc we collect the rest of
//! the sequence and decode it; a complete sequence we don't know is
//! dropped, never typed into the app.

use std::{io, time::Duration};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

/// How long to wait for each further byte of a split sequence. A real Esc
/// press has nothing following it within this time.
pub const WAIT: Duration = Duration::from_millis(50);
/// Longest CSI sequence we collect before giving up.
const MAX_LEN: usize = 32;

fn key(e: &Event) -> Option<&KeyEvent> {
    match e {
        Event::Key(k) if k.kind == KeyEventKind::Press => Some(k),
        _ => None,
    }
}

fn char_of(e: &Event) -> Option<char> {
    match key(e)?.code {
        KeyCode::Char(c) => Some(c),
        _ => None,
    }
}

fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// The key a final character (CSI or SS3) stands for.
fn final_key(c: char) -> Option<KeyCode> {
    Some(match c {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        _ => return None,
    })
}

/// Decodes the body of a CSI sequence (after `ESC [`, final character
/// included): an SGR mouse event, an arrow or Home/End (modifiers ignored),
/// or a `~` key. `None` for anything else.
fn decode_csi(body: &str) -> Option<Event> {
    let last = body.chars().last()?;
    let params = &body[..body.len() - last.len_utf8()];
    if let Some(mouse) = params.strip_prefix('<') {
        let mut n = mouse.split(';').map(|p| p.parse::<u16>().ok());
        let (b, x, y) = (n.next()??, n.next()??, n.next()??);
        // Bits 2-4 are Shift, Alt and Ctrl; the rest is the button.
        let kind = match (b & !0b0001_1100, last) {
            (64, _) => MouseEventKind::ScrollUp,
            (65, _) => MouseEventKind::ScrollDown,
            (0, 'M') => MouseEventKind::Down(MouseButton::Left),
            (1, 'M') => MouseEventKind::Down(MouseButton::Middle),
            (2, 'M') => MouseEventKind::Down(MouseButton::Right),
            (0, 'm') => MouseEventKind::Up(MouseButton::Left),
            (1, 'm') => MouseEventKind::Up(MouseButton::Middle),
            (2, 'm') => MouseEventKind::Up(MouseButton::Right),
            _ => return None,
        };
        return Some(Event::Mouse(MouseEvent {
            kind,
            column: x.saturating_sub(1),
            row: y.saturating_sub(1),
            modifiers: KeyModifiers::NONE,
        }));
    }
    if let Some(code) = final_key(last) {
        return Some(press(code));
    }
    if last == '~' {
        let code = match params.split(';').next()? {
            "1" | "7" => KeyCode::Home,
            "2" => KeyCode::Insert,
            "3" => KeyCode::Delete,
            "4" | "8" => KeyCode::End,
            "5" => KeyCode::PageUp,
            "6" => KeyCode::PageDown,
            _ => return None,
        };
        return Some(press(code));
    }
    None
}

/// The events `first` really stands for. After a lone Esc it asks `next`
/// (waiting at most [`WAIT`] each time) for what follows and rebuilds a
/// split sequence; anything that isn't one is passed on as it came.
pub fn reassemble(
    first: Event,
    mut next: impl FnMut(Duration) -> io::Result<Option<Event>>,
) -> io::Result<Vec<Event>> {
    let lone_esc = key(&first).is_some_and(|k| k.code == KeyCode::Esc && k.modifiers.is_empty());
    if !lone_esc {
        return Ok(vec![first]);
    }
    let Some(second) = next(WAIT)? else {
        return Ok(vec![first]);
    };
    match char_of(&second) {
        // SS3: one final character.
        Some('O') => {
            let Some(third) = next(WAIT)? else {
                return Ok(vec![first, second]);
            };
            Ok(match char_of(&third).and_then(final_key) {
                Some(code) => vec![press(code)],
                None => vec![first, second, third],
            })
        }
        // CSI: parameters, then a final character in @..~ ('<' only opens
        // an SGR mouse sequence).
        Some('[') => {
            let mut seen = vec![first, second];
            let mut body = String::new();
            while body.len() < MAX_LEN {
                let Some(e) = next(WAIT)? else {
                    // Cut off: pass on what we have.
                    return Ok(seen);
                };
                let Some(c) = char_of(&e) else {
                    seen.push(e);
                    return Ok(seen);
                };
                seen.push(e);
                body.push(c);
                let opens_mouse = body.len() == 1 && c == '<';
                if ('@'..='~').contains(&c) && !opens_mouse {
                    // A complete sequence: decoded, or dropped.
                    return Ok(decode_csi(&body).into_iter().collect());
                }
            }
            Ok(Vec::new())
        }
        _ => Ok(vec![first, second]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `reassemble` with these characters waiting after the first event.
    fn run(first: Event, rest: &str) -> Vec<Event> {
        let mut rest = rest.chars().map(|c| press(KeyCode::Char(c)));
        reassemble(first, |_| Ok(rest.next())).unwrap()
    }

    fn esc() -> Event {
        press(KeyCode::Esc)
    }

    #[test]
    fn split_keys_are_rebuilt() {
        for (rest, code) in [
            ("[A", KeyCode::Up),
            ("[D", KeyCode::Left),
            ("OB", KeyCode::Down),
            ("[H", KeyCode::Home),
            ("[1;5C", KeyCode::Right),
            ("[5~", KeyCode::PageUp),
            ("[6~", KeyCode::PageDown),
        ] {
            assert_eq!(run(esc(), rest), [press(code)], "{rest}");
        }
    }

    #[test]
    fn split_mouse_events_are_rebuilt_not_typed() {
        let wheel = run(esc(), "[<65;12;7M");
        assert_eq!(
            wheel,
            [Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 11,
                row: 6,
                modifiers: KeyModifiers::NONE,
            })]
        );
        let click = run(esc(), "[<0;3;1M");
        assert!(matches!(
            click[..],
            [Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                row: 0,
                ..
            })]
        ));
        // Motion and other unknown sequences are dropped, not typed: no
        // digits reach the app to switch tabs.
        assert!(run(esc(), "[<35;40;12M").is_empty());
        assert!(run(esc(), "[?1;2c").is_empty());
    }

    #[test]
    fn real_keys_pass_through() {
        // A real Esc: nothing follows.
        assert_eq!(run(esc(), ""), [esc()]);
        // Esc, then an unrelated key.
        assert_eq!(run(esc(), "q"), [esc(), press(KeyCode::Char('q'))]);
        // A sequence cut off is passed on as it came.
        assert_eq!(
            run(esc(), "[1"),
            [esc(), press(KeyCode::Char('[')), press(KeyCode::Char('1'))]
        );
        // Anything that isn't a lone Esc is left alone.
        assert_eq!(run(press(KeyCode::Up), ""), [press(KeyCode::Up)]);
    }
}
