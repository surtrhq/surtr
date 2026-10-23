//! Two runs side by side, for example before and after a tuning change
//! (plan, section 6.1, `compare_runs`).

use serde::Serialize;

use crate::{
    html::verdict,
    record::{Record, Tick},
};

/// One run, reduced to what is worth comparing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Run id.
    pub id: String,
    /// The verdict, in words.
    pub result: String,
    /// How long it ran, in milliseconds.
    pub length_ms: u64,
    /// Worker threads.
    pub threads: u32,
    /// Their CPUs.
    pub cpus: Vec<u32>,
    /// Verified iterations per second, all threads together.
    pub rate: Option<f64>,
    /// Iterations per second of each thread.
    pub thread_rates: Vec<f64>,
    /// Highest and average watched temperature, in °C.
    pub peak_temp_c: Option<f64>,
    /// Average watched temperature, in °C.
    pub avg_temp_c: Option<f64>,
    /// How far throughput sagged: the slowest tenth of the seconds against
    /// the median, in percent (0 is steady; a few percent is noise; more
    /// means throttling).
    pub sag_pct: Option<f64>,
    /// Wrong answers.
    pub errors: u32,
}

/// Two runs and how the second differs from the first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// The first run (the baseline).
    pub a: Summary,
    /// The second run.
    pub b: Summary,
    /// Throughput change from a to b, in percent.
    pub rate_change_pct: Option<f64>,
    /// Peak temperature change, in °C.
    pub peak_temp_change_c: Option<f64>,
    /// Average temperature change, in °C.
    pub avg_temp_change_c: Option<f64>,
    /// Why the numbers may not be comparable.
    pub warnings: Vec<String>,
}

/// Per-second rates from the timeline.
fn rates(timeline: &[Tick]) -> Vec<f64> {
    timeline
        .windows(2)
        .filter_map(|w| {
            let dt = w[1].elapsed_ms.checked_sub(w[0].elapsed_ms)? as f64 / 1000.0;
            (dt > 0.0).then(|| w[1].iterations.saturating_sub(w[0].iterations) as f64 / dt)
        })
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted.get(i).copied()
}

/// The summary of one run.
pub fn summary(r: &Record) -> Summary {
    let length_ms = r.report.as_ref().map_or_else(
        || r.timeline.last().map_or(0, |t| t.elapsed_ms),
        |rep| rep.duration_ms,
    );
    let secs = length_ms as f64 / 1000.0;
    let iterations: Vec<u64> = r
        .report
        .as_ref()
        .map(|rep| rep.iterations.clone())
        .unwrap_or_default();
    let total: u64 = iterations.iter().sum();
    let total = if total == 0 {
        r.timeline.last().map_or(0, |t| t.iterations)
    } else {
        total
    };
    let temps: Vec<f64> = r.timeline.iter().filter_map(|t| t.temp_c).collect();
    let mut per_second = rates(&r.timeline);
    per_second.sort_by(f64::total_cmp);
    let sag_pct = match (percentile(&per_second, 0.1), percentile(&per_second, 0.5)) {
        (Some(low), Some(mid)) if mid > 0.0 => Some(((1.0 - low / mid) * 100.0).max(0.0)),
        _ => None,
    };
    Summary {
        id: r.id.clone(),
        result: verdict(r).0,
        length_ms,
        threads: r.threads,
        cpus: r.cpus.clone(),
        rate: (secs > 0.0 && total > 0).then(|| total as f64 / secs),
        thread_rates: if secs > 0.0 {
            iterations.iter().map(|&n| n as f64 / secs).collect()
        } else {
            Vec::new()
        },
        peak_temp_c: r
            .report
            .as_ref()
            .and_then(|rep| rep.peak_temp_c)
            .or_else(|| temps.iter().copied().reduce(f64::max)),
        avg_temp_c: (!temps.is_empty()).then(|| temps.iter().sum::<f64>() / temps.len() as f64),
        sag_pct,
        errors: r.report.as_ref().map_or(r.errors_seen, |rep| {
            u32::try_from(rep.failures.len()).unwrap_or(u32::MAX)
        }),
    }
}

/// Compares `b` against the baseline `a`.
pub fn compare(a: &Record, b: &Record) -> Comparison {
    let (sa, sb) = (summary(a), summary(b));
    let mut warnings = Vec::new();
    if a.workload != b.workload {
        warnings.push(format!(
            "different tests: {} and {}",
            a.workload, b.workload
        ));
    }
    if a.threads != b.threads || (!a.cpus.is_empty() && !b.cpus.is_empty() && a.cpus != b.cpus) {
        warnings.push("different threads or CPUs; compare per-thread rates".to_owned());
    }
    if a.machine != b.machine {
        warnings.push("recorded on different machines (or the OS changed)".to_owned());
    }
    if a.surtr != b.surtr {
        warnings.push(format!(
            "different Surtr versions ({} and {}); kernels may differ",
            a.surtr, b.surtr
        ));
    }
    let short = |s: &Summary| s.length_ms < 30_000;
    if short(&sa) || short(&sb) {
        warnings.push("a run under 30 s says little about temperature".to_owned());
    }
    Comparison {
        rate_change_pct: match (sa.rate, sb.rate) {
            (Some(x), Some(y)) if x > 0.0 => Some((y / x - 1.0) * 100.0),
            _ => None,
        },
        peak_temp_change_c: sa.peak_temp_c.zip(sb.peak_temp_c).map(|(x, y)| y - x),
        avg_temp_change_c: sa.avg_temp_c.zip(sb.avg_temp_c).map(|(x, y)| y - x),
        a: sa,
        b: sb,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record::{SCHEMA, State},
        runner::{Outcome, Report},
    };

    fn run(id: &str, rate: u64, temp: f64, sag_at: Option<u64>) -> Record {
        let timeline: Vec<Tick> = (0..=60u64)
            .map(|s| {
                // A throttled stretch runs at half speed.
                let slow = sag_at.is_some_and(|from| s > from);
                Tick {
                    elapsed_ms: s * 1000,
                    temp_c: Some(temp + s as f64 / 10.0),
                    iterations: (0..s)
                        .map(|i| {
                            if sag_at.is_some_and(|f| i >= f) && slow {
                                rate / 2
                            } else {
                                rate
                            }
                        })
                        .sum(),
                }
            })
            .collect();
        let total = timeline.last().unwrap().iterations;
        Record {
            schema: SCHEMA,
            id: id.into(),
            workload: "cpu".into(),
            state: State::Done,
            started: "2026-10-23T14:12:00Z".into(),
            ended: None,
            planned_ms: 60_000,
            threads: 2,
            cpus: vec![0, 2],
            surtr: "0.2.0".into(),
            machine: vec!["Ryzen".into()],
            watched: Vec::new(),
            requested_by: None,
            errors_seen: 0,
            timeline,
            report: Some(Report {
                outcome: Outcome::Passed,
                threads: 2,
                duration_ms: 60_000,
                iterations: vec![total / 2, total / 2],
                failures: Vec::new(),
                peak_temp_c: Some(temp + 6.0),
                watched: Vec::new(),
            }),
        }
    }

    #[test]
    fn faster_and_cooler() {
        let c = compare(&run("a", 1000, 70.0, None), &run("b", 1100, 65.0, None));
        assert!((c.rate_change_pct.unwrap() - 10.0).abs() < 0.01);
        assert_eq!(c.peak_temp_change_c, Some(-5.0));
        assert!((c.avg_temp_change_c.unwrap() + 5.0).abs() < 0.01);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.a.thread_rates.len(), 2);
        assert!(c.a.sag_pct.unwrap() < 1.0, "steady");
        assert_eq!(c.b.result, "Passed: no errors");
    }

    #[test]
    fn throttling_shows_as_sag() {
        let s = summary(&run("t", 1000, 80.0, Some(40)));
        assert!(s.sag_pct.unwrap() > 40.0, "{:?}", s.sag_pct);
    }

    #[test]
    fn mismatches_are_flagged() {
        let a = run("a", 1000, 70.0, None);
        let mut b = run("b", 1000, 70.0, None);
        b.threads = 8;
        b.cpus = (0..8).collect();
        b.surtr = "0.3.0".into();
        let c = compare(&a, &b);
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
    }
}
