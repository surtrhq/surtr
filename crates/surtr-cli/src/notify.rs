//! Desktop notifications for alerts raised by `surtr serve` (plan 5.2.1).
//! Only fired alerts notify; a failed notification is logged and never stops
//! the daemon.

use std::thread;

use surtr_core::alerts::{Event, Severity, State};

/// Title and body of the notification for `event`, if it should notify.
pub fn message(event: &Event) -> Option<(String, String)> {
    if event.state != State::Fired {
        return None;
    }
    let word = match event.severity {
        Severity::Warning => "high",
        Severity::Critical => "critical",
    };
    Some((
        format!("Surtr: {} is {word}", event.label),
        format!("{} at {:.1} °C", event.device, event.value),
    ))
}

/// Shows the notification for `event` without blocking the caller.
pub fn send(event: &Event) {
    let Some((title, body)) = message(event) else {
        return;
    };
    thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .appname("Surtr")
            .summary(&title)
            .body(&body)
            .show();
        if let Err(err) = result {
            tracing::warn!("desktop notification failed: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_model::Timestamp;

    fn event(state: State) -> Event {
        Event {
            at: Timestamp(0),
            rule: "critical-temperature/nvidia/nvidia0/temp".to_owned(),
            state,
            severity: Severity::Critical,
            provider: "nvidia".to_owned(),
            sensor: "nvidia0/temp".to_owned(),
            device: "nvidia0".to_owned(),
            label: "GPU temperature".to_owned(),
            value: 92.0,
        }
    }

    #[test]
    fn fired_alerts_notify() {
        assert_eq!(
            message(&event(State::Fired)),
            Some((
                "Surtr: GPU temperature is critical".to_owned(),
                "nvidia0 at 92.0 °C".to_owned()
            ))
        );
    }

    #[test]
    fn resolved_alerts_do_not() {
        assert_eq!(message(&event(State::Resolved)), None);
    }
}
