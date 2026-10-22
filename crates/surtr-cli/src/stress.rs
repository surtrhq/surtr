//! `surtr stress cpu`: the host side of the built-in CPU test (plan, section
//! 5.3 and M6). The test itself runs in a `surtr worker` process; this module
//! picks the limits, watches the temperatures, and reports the result.

use std::{
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use surtr_core::Hub;
use surtr_stress::{
    event::Event,
    runner::{self, Outcome, Report, Watched},
};

/// What the user asked for.
#[derive(Debug)]
pub struct Options {
    /// How long to run.
    pub duration: Duration,
    /// Longest allowed run (`stress.max_duration`).
    pub max_duration: Duration,
    /// Temperature limit in °C (`stress.max_temp_c`).
    pub max_temp_c: f64,
    /// Worker threads.
    pub threads: u32,
    /// Allow a short run without a temperature sensor.
    pub unmonitored: bool,
}

/// Reads temperatures and clocks through the hub.
struct HubSensors<'a> {
    hub: &'a Hub,
    watched: Vec<String>,
    clocks: Vec<(u32, f64)>,
}

impl runner::Sensors for HubSensors<'_> {
    fn temperatures(&mut self) -> Vec<(String, f64)> {
        let samples = self.hub.read();
        self.clocks = samples
            .iter()
            .filter_map(|s| {
                let cpu = s
                    .descriptor
                    .id
                    .0
                    .strip_prefix("cpu/thread")?
                    .strip_suffix("/clock")?
                    .parse()
                    .ok()?;
                Some((cpu, s.value?))
            })
            .collect();
        samples
            .into_iter()
            .filter(|s| self.watched.contains(&s.descriptor.id.0))
            .filter_map(|s| Some((s.descriptor.id.0, s.value?)))
            .collect()
    }

    fn clock(&mut self, cpu: u32) -> Option<f64> {
        self.clocks.iter().find(|(c, _)| *c == cpu).map(|(_, v)| *v)
    }
}

/// `1m 05s` for a number of milliseconds.
fn span(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Runs the test; returns the report and the exit code (0 passed, 2 stopped
/// early, 3 errors found or the worker failed).
pub fn run(hub: &Hub, opts: &Options) -> Result<(Report, u8), String> {
    if opts.duration > opts.max_duration {
        return Err(format!(
            "{} is longer than the {} limit (stress.max_duration in the settings)",
            span(u64::try_from(opts.duration.as_millis()).unwrap_or(u64::MAX)),
            span(u64::try_from(opts.max_duration.as_millis()).unwrap_or(u64::MAX)),
        ));
    }
    let watched: Vec<Watched> = runner::watched(&hub.sensors(), opts.max_temp_c);
    let mut duration = opts.duration;
    if watched.is_empty() {
        if !opts.unmonitored {
            return Err(
                "no CPU temperature sensor to watch, so a stress test could overheat this \
                 machine unnoticed; `surtr caps` shows why it is missing. Pass --unmonitored \
                 for a 60-second check without temperature protection."
                    .to_owned(),
            );
        }
        duration = duration.min(runner::UNMONITORED_MAX);
        eprintln!(
            "surtr: warning: no CPU temperature sensor; running {} without temperature protection",
            span(u64::try_from(duration.as_millis()).unwrap_or(0))
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // A second Ctrl+C after the first falls through to the default.
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }
    let mut worker = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    worker.args([
        "worker",
        "cpu",
        "--threads",
        &opts.threads.to_string(),
        "--duration-ms",
        &duration.as_millis().to_string(),
    ]);
    let limits: Vec<String> = watched
        .iter()
        .map(|w| format!("{} {:.0} °C", w.label, w.stop_at))
        .collect();
    eprintln!(
        "Stressing {} threads for {}; stops at {}. Press Ctrl+C to stop.",
        opts.threads,
        span(u64::try_from(duration.as_millis()).unwrap_or(0)),
        if limits.is_empty() {
            "the time limit".to_owned()
        } else {
            limits.join(", ")
        }
    );
    let live = std::io::stderr().is_terminal();
    let mut sensors = HubSensors {
        hub,
        watched: watched.iter().map(|w| w.id.clone()).collect(),
        clocks: Vec::new(),
    };
    let report = runner::run(
        worker,
        duration,
        &watched,
        &mut sensors,
        &stop,
        |event, temp| {
            if let (true, Event::Progress { elapsed_ms, .. }) = (live, event) {
                let temp = temp.map_or_else(String::new, |t| format!(" · {t:.1} °C"));
                eprint!("\r  {}{temp}    ", span(*elapsed_ms));
                let _ = std::io::stderr().flush();
            }
        },
    )?;
    if live {
        eprintln!();
    }
    let code = match report.outcome {
        Outcome::Passed => 0,
        Outcome::TemperatureLimit { .. } | Outcome::Stopped => 2,
        Outcome::ErrorsFound | Outcome::WorkerFailed { .. } => 3,
    };
    Ok((report, code))
}

/// The report as text.
pub fn describe(r: &Report) -> String {
    let mut out = String::new();
    let verdict = match &r.outcome {
        Outcome::Passed => "PASSED: no errors".to_owned(),
        Outcome::ErrorsFound => format!(
            "ERRORS FOUND: the CPU gave {} wrong answer(s); it is not stable at these settings",
            r.failures.len()
        ),
        Outcome::TemperatureLimit {
            sensor,
            value,
            limit,
        } => format!("STOPPED: {sensor} reached {value:.1} °C (limit {limit:.0} °C)"),
        Outcome::Stopped => "STOPPED by you".to_owned(),
        Outcome::WorkerFailed { reason } => format!("FAILED: {reason}"),
    };
    let _ = writeln!(out, "{verdict}");
    let total: u64 = r.iterations.iter().sum();
    let _ = writeln!(
        out,
        "  {} threads · {} · {total} iterations{}",
        r.threads,
        span(r.duration_ms),
        r.peak_temp_c
            .map_or_else(String::new, |t| format!(" · peak {t:.1} °C")),
    );
    for f in &r.failures {
        let where_ = f
            .cpu
            .map_or_else(|| format!("thread {}", f.thread), |c| format!("CPU {c}"));
        let clock = f
            .clock_mhz
            .map_or_else(String::new, |c| format!(" at {c:.0} MHz"));
        let _ = writeln!(
            out,
            "  ! {where_}: wrong {} result after {}{clock}",
            f.kernel,
            span(f.elapsed_ms)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        assert_eq!(span(4_200), "4s");
        assert_eq!(span(65_000), "1m 05s");
        assert_eq!(span(7_200_000), "2h 00m");
    }
}
