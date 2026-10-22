//! Alert rules and their evaluation (plan, sections 5.2 and 5.2.1).
//!
//! A rule watches sensors that match it and fires when its condition has held
//! for `for_ms`; it resolves when the condition stops holding. After firing it
//! stays quiet for `quiet_ms` before it may fire again for the same sensor.
//! The engine is a pure state machine: give it samples and the time, get
//! events back. Delivering them (log, notifications) is the caller's job.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Sample;
use surtr_model::{SensorKind, Timestamp};

/// How serious an alert is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth a look.
    Warning,
    /// Act now.
    Critical,
}

/// When a rule's condition holds for a value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// Value at or above the threshold.
    Above(f64),
    /// Value at or below the threshold.
    Below(f64),
}

impl Condition {
    fn holds(self, value: f64) -> bool {
        match self {
            Self::Above(t) => value >= t,
            Self::Below(t) => value <= t,
        }
    }
}

/// One alert rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable name, for example `cpu-temperature-critical`.
    pub name: String,
    /// Only this provider's sensors.
    #[serde(default)]
    pub provider: Option<String>,
    /// Only the sensor with exactly this id.
    #[serde(default)]
    pub sensor: Option<String>,
    /// Only sensors of this kind.
    pub kind: Option<SensorKind>,
    /// Only sensors whose device starts with this, for example `cpu`.
    pub device_prefix: Option<String>,
    /// Only sensors whose label contains this (case-insensitive).
    pub label_contains: Option<String>,
    /// When it fires.
    pub condition: Condition,
    /// How long the condition must hold, in milliseconds.
    pub for_ms: u64,
    /// How serious it is.
    pub severity: Severity,
    /// After firing, no new fire for the same sensor for this long.
    pub quiet_ms: u64,
}

impl Rule {
    fn matches(&self, s: &Sample) -> bool {
        let d = &s.descriptor;
        self.provider
            .as_deref()
            .is_none_or(|p| p == d.source.provider)
            && self.sensor.as_deref().is_none_or(|id| id == d.id.0)
            && self.kind.is_none_or(|k| k == d.kind)
            && self
                .device_prefix
                .as_deref()
                .is_none_or(|p| d.device.starts_with(p))
            && self
                .label_contains
                .as_deref()
                .is_none_or(|t| d.label.to_lowercase().contains(&t.to_lowercase()))
    }
}

/// Something that changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// When it happened.
    pub at: Timestamp,
    /// The rule.
    pub rule: String,
    /// Fired or resolved.
    pub state: State,
    /// Rule severity.
    pub severity: Severity,
    /// The sensor's provider.
    pub provider: String,
    /// The sensor id.
    pub sensor: String,
    /// The sensor's device.
    pub device: String,
    /// The sensor's label.
    pub label: String,
    /// The value that triggered the change.
    pub value: f64,
}

/// Whether an alert started or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The condition held long enough.
    Fired,
    /// The condition no longer holds.
    Resolved,
}

#[derive(Debug, Default)]
struct Track {
    /// When the condition started holding.
    since: Option<Timestamp>,
    /// Whether it has fired and not yet resolved.
    active: bool,
    /// When it last fired.
    last_fired: Option<Timestamp>,
}

/// Evaluates rules over successive snapshots.
#[derive(Debug, Default)]
pub struct Engine {
    rules: Vec<Rule>,
    tracks: HashMap<(usize, String, String), Track>,
}

impl Engine {
    /// An engine for `rules`.
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            tracks: HashMap::new(),
        }
    }

    /// The rules.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Feeds one snapshot taken at `now`; returns what fired or resolved.
    /// Sensors without a value this time keep their state.
    pub fn evaluate(&mut self, samples: &[Sample], now: Timestamp) -> Vec<Event> {
        let mut events = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            for s in samples.iter().filter(|s| rule.matches(s)) {
                let Some(value) = s.value else { continue };
                let d = &s.descriptor;
                let key = (i, d.source.provider.clone(), d.id.0.clone());
                let track = self.tracks.entry(key).or_default();
                let event = |state| Event {
                    at: now,
                    rule: rule.name.clone(),
                    state,
                    severity: rule.severity,
                    provider: d.source.provider.clone(),
                    sensor: d.id.0.clone(),
                    device: d.device.clone(),
                    label: d.label.clone(),
                    value,
                };
                if rule.condition.holds(value) {
                    let since = *track.since.get_or_insert(now);
                    let held = now.0.saturating_sub(since.0) >= rule.for_ms;
                    let quiet = track
                        .last_fired
                        .is_some_and(|t| now.0.saturating_sub(t.0) < rule.quiet_ms);
                    if held && !track.active && !quiet {
                        track.active = true;
                        track.last_fired = Some(now);
                        events.push(event(State::Fired));
                    }
                } else {
                    track.since = None;
                    if track.active {
                        track.active = false;
                        events.push(event(State::Resolved));
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_model::{SensorDescriptor, SensorId, Source};

    fn temp(value: f64) -> Sample {
        Sample {
            descriptor: SensorDescriptor {
                id: SensorId::new("cpu0/temp"),
                kind: SensorKind::Temperature,
                unit: SensorKind::Temperature.unit(),
                device: "cpu0".to_owned(),
                label: "Package temperature".to_owned(),
                source: Source::new("test", "test"),
            },
            value: Some(value),
            at: None,
        }
    }

    fn rule() -> Rule {
        Rule {
            name: "cpu-hot".to_owned(),
            provider: None,
            sensor: None,
            kind: Some(SensorKind::Temperature),
            device_prefix: Some("cpu".to_owned()),
            label_contains: None,
            condition: Condition::Above(90.0),
            for_ms: 3000,
            severity: Severity::Critical,
            quiet_ms: 60_000,
        }
    }

    fn states(engine: &mut Engine, steps: &[(u64, f64)]) -> Vec<(u64, State)> {
        steps
            .iter()
            .flat_map(|&(t, v)| {
                engine
                    .evaluate(&[temp(v)], Timestamp(t))
                    .into_iter()
                    .map(move |e| (t, e.state))
            })
            .collect()
    }

    #[test]
    fn fires_after_holding_and_resolves() {
        let mut e = Engine::new(vec![rule()]);
        let got = states(
            &mut e,
            &[
                (0, 95.0),
                (1000, 95.0),
                (3000, 96.0),
                (4000, 97.0),
                (5000, 80.0),
            ],
        );
        assert_eq!(got, [(3000, State::Fired), (5000, State::Resolved)]);
    }

    #[test]
    fn short_spikes_do_not_fire() {
        let mut e = Engine::new(vec![rule()]);
        let got = states(
            &mut e,
            &[(0, 95.0), (1000, 80.0), (2000, 95.0), (4000, 80.0)],
        );
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn quiet_period_suppresses_refiring() {
        let mut e = Engine::new(vec![rule()]);
        let got = states(
            &mut e,
            &[
                (0, 95.0),
                (3000, 95.0),
                (4000, 80.0),
                (5000, 95.0),
                (9000, 95.0),
                (70_000, 95.0),
            ],
        );
        // Fires at 3 s, resolves at 4 s; holding again from 5 s would fire
        // at 8 s but the quiet period runs until 63 s.
        assert_eq!(
            got,
            [
                (3000, State::Fired),
                (4000, State::Resolved),
                (70_000, State::Fired)
            ]
        );
    }

    #[test]
    fn rules_only_match_their_sensors() {
        let mut gpu_rule = rule();
        gpu_rule.device_prefix = Some("gpu".to_owned());
        gpu_rule.for_ms = 0;
        let mut e = Engine::new(vec![gpu_rule]);
        assert!(e.evaluate(&[temp(99.0)], Timestamp(0)).is_empty());
    }
}
