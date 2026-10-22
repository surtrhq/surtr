//! Arrow keys whose escape sequence arrived in pieces. Over SSH the bytes of
//! an arrow key (`ESC [ A`) can be split across packets; crossterm then
//! reports a lone Esc followed by the characters `[` and `A`. Seen on a
//! remote session, where it used to quit the TUI.

use std::{io, time::Duration};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// How long to wait for the rest of a split sequence. A real Esc press has
/// nothing following it within this time.
pub const WAIT: Duration = Duration::from_millis(50);

fn key(e: &Event) -> Option<&KeyEvent> {
    match e {
        Event::Key(k) if k.kind == KeyEventKind::Press => Some(k),
        _ => None,
    }
}

/// The key a CSI or SS3 sequence's final character stands for.
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

/// The events `first` really stands for. After a lone Esc it asks `next`
/// (waiting at most [`WAIT`]) for what follows, and turns `[` or `O` plus a
/// final character back into the arrow key; anything else is passed on as
/// it came.
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
    let introducer = key(&second).is_some_and(|k| matches!(k.code, KeyCode::Char('[' | 'O')));
    if !introducer {
        return Ok(vec![first, second]);
    }
    let Some(third) = next(WAIT)? else {
        return Ok(vec![first, second]);
    };
    let code = key(&third).and_then(|k| match k.code {
        KeyCode::Char(c) => final_key(c),
        _ => None,
    });
    Ok(match code {
        Some(code) => vec![Event::Key(KeyEvent::new(code, KeyModifiers::NONE))],
        None => vec![first, second, third],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// `reassemble` with the following events already waiting.
    fn run(first: Event, rest: &[Event]) -> Vec<Event> {
        let mut rest = rest.iter().cloned();
        reassemble(first, |_| Ok(rest.next())).unwrap()
    }

    #[test]
    fn split_arrow_keys_are_rebuilt() {
        let esc = press(KeyCode::Esc);
        for (c, code) in [
            ('A', KeyCode::Up),
            ('D', KeyCode::Left),
            ('H', KeyCode::Home),
        ] {
            assert_eq!(
                run(
                    esc.clone(),
                    &[press(KeyCode::Char('[')), press(KeyCode::Char(c))]
                ),
                [press(code)]
            );
        }
        // SS3 form, as some terminals send in application mode.
        assert_eq!(
            run(esc, &[press(KeyCode::Char('O')), press(KeyCode::Char('B'))]),
            [press(KeyCode::Down)]
        );
    }

    #[test]
    fn real_keys_pass_through() {
        let esc = press(KeyCode::Esc);
        // A real Esc: nothing follows.
        assert_eq!(run(esc.clone(), &[]), std::slice::from_ref(&esc));
        // Esc, then an unrelated key.
        let q = press(KeyCode::Char('q'));
        assert_eq!(run(esc.clone(), std::slice::from_ref(&q)), [esc.clone(), q]);
        // Esc and '[' with no final character, or a different one.
        let bracket = press(KeyCode::Char('['));
        assert_eq!(
            run(esc.clone(), std::slice::from_ref(&bracket)),
            [esc.clone(), bracket.clone()]
        );
        let x = press(KeyCode::Char('x'));
        assert_eq!(
            run(esc.clone(), &[bracket.clone(), x.clone()]),
            [esc, bracket, x]
        );
        // Anything that isn't a lone Esc is left alone.
        assert_eq!(run(press(KeyCode::Up), &[]), [press(KeyCode::Up)]);
    }
}
