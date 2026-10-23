//! Load patterns: steady, or switching on and off (`--pattern transient`).
//!
//! Steady all-core load never leaves boost; many unstable per-core settings
//! (an aggressive Curve Optimizer offset, say) crash instead when a core
//! wakes from deep idle and jumps to full boost, or when the voltage
//! regulator has to follow a sudden load step. The transient pattern
//! provokes exactly that: all threads switch together (the biggest current
//! steps), with load bursts and short gaps of varying length, and every
//! tenth gap long enough for cores to fall into deep idle states.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How the load is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Full load all the time.
    #[default]
    Steady,
    /// Load switching on and off.
    Transient,
}

impl Pattern {
    /// Its name, as in `--pattern`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Steady => "steady",
            Self::Transient => "transient",
        }
    }
}

/// Load burst lengths, in milliseconds.
const ON_MS: (u64, u64) = (30, 800);
/// Short gaps, in milliseconds.
const OFF_MS: (u64, u64) = (10, 400);
/// Every this many cycles, a gap long enough for deep idle states.
const LONG_EVERY: usize = 10;
/// The long gap, in milliseconds.
const LONG_OFF_MS: u64 = 1500;

/// The on/off schedule: alternating load and idle phases, the same for every
/// thread and every run (a fixed seed), so runs stay comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    /// Where each phase ends, in milliseconds from the start; even phases
    /// load, odd ones idle.
    ends: Vec<u64>,
}

impl Schedule {
    /// A schedule covering `duration`.
    pub fn new(duration: Duration) -> Self {
        let total = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = |(lo, hi): (u64, u64)| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            lo + x % (hi - lo + 1)
        };
        let mut ends = Vec::new();
        let mut at = 0u64;
        let mut cycle = 0usize;
        while at < total {
            at += next(ON_MS);
            ends.push(at);
            cycle += 1;
            at += if cycle.is_multiple_of(LONG_EVERY) {
                LONG_OFF_MS
            } else {
                next(OFF_MS)
            };
            ends.push(at);
        }
        Self { ends }
    }

    /// At `elapsed_ms`: `None` while loading, or how long the current idle
    /// phase still lasts.
    pub fn idle_for(&self, elapsed_ms: u64) -> Option<Duration> {
        let phase = self.ends.partition_point(|&end| end <= elapsed_ms);
        if phase % 2 == 0 {
            return None;
        }
        let end = self.ends.get(phase).copied().unwrap_or(elapsed_ms);
        Some(Duration::from_millis(end - elapsed_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_alternates_with_long_gaps() {
        let s = Schedule::new(Duration::from_secs(60));
        assert!(s.ends.last().copied().unwrap() >= 60_000, "covers the run");
        assert_eq!(s, Schedule::new(Duration::from_secs(60)), "same every run");
        // Load at the start; idle somewhere; long idles present.
        assert_eq!(s.idle_for(0), None);
        let mut idle = 0u64;
        let mut longest = Duration::ZERO;
        let mut t = 0u64;
        while t < 60_000 {
            if let Some(left) = s.idle_for(t) {
                idle += 1;
                longest = longest.max(left);
            }
            t += 5;
        }
        let share = idle as f64 / 12_000.0;
        assert!((0.2..0.6).contains(&share), "idle share {share}");
        assert!(longest >= Duration::from_millis(1400), "{longest:?}");
    }
}
