//! The host side of a stress run: starts the worker process, reads its
//! events, watches the temperature limit, and stops the worker on a limit,
//! at the time cap or when the user stops it. A worker that crashes, hangs or
//! is killed by the OS is a result, never a host failure (plan, section 4).

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use surtr_model::{SensorDescriptor, SensorKind};

use crate::event::Event;

/// Default temperature limit in °C (decided for M6).
pub const DEFAULT_MAX_TEMP_C: f64 = 95.0;
/// How far below a CPU's own critical limit a run stops, in °C.
pub const CRITICAL_MARGIN_C: f64 = 5.0;
/// Default cap on a run's length.
pub const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(30 * 60);
/// Longest run allowed without a temperature sensor to watch.
pub const UNMONITORED_MAX: Duration = Duration::from_secs(60);

/// A temperature the host watches, and where it stops the run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Watched {
    /// Sensor id, for example `hwmon/cpu0/temp1`.
    pub id: String,
    /// Its label, for example `Tctl`.
    pub label: String,
    /// The run stops at or above this temperature, in °C.
    pub stop_at: f64,
}

/// The CPU temperatures to watch and their stop limits: `max_temp_c`, or
/// [`CRITICAL_MARGIN_C`] below a sensor's own critical limit if lower.
pub fn watched(sensors: &[SensorDescriptor], max_temp_c: f64) -> Vec<Watched> {
    sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Temperature && s.device.starts_with("cpu"))
        .map(|s| Watched {
            id: s.id.0.clone(),
            label: s.label.clone(),
            stop_at: s
                .limits
                .critical
                .map_or(max_temp_c, |c| max_temp_c.min(c - CRITICAL_MARGIN_C)),
        })
        .collect()
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    /// Ran its whole duration without a wrong answer.
    Passed,
    /// The hardware gave wrong answers.
    ErrorsFound,
    /// Stopped because a temperature reached its limit.
    TemperatureLimit {
        /// The sensor's label.
        sensor: String,
        /// The temperature read, in °C.
        value: f64,
        /// The limit, in °C.
        limit: f64,
    },
    /// Stopped by the user.
    Stopped,
    /// The worker ended without finishing: crashed, killed by the OS, or
    /// stopped answering.
    WorkerFailed {
        /// What happened.
        reason: String,
    },
}

/// A wrong answer, with what the host knew at the time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    /// Worker thread.
    pub thread: u32,
    /// Logical CPU, if the thread was pinned.
    pub cpu: Option<u32>,
    /// The kernel that failed.
    pub kernel: String,
    /// Milliseconds into the run.
    pub elapsed_ms: u64,
    /// That CPU's clock at the last read before the error, in MHz.
    pub clock_mhz: Option<f64>,
}

/// The result of a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// How it ended. Wrong answers win over a later limit or stop.
    pub outcome: Outcome,
    /// Worker threads.
    pub threads: u32,
    /// How long it ran, in milliseconds.
    pub duration_ms: u64,
    /// Iterations per thread.
    pub iterations: Vec<u64>,
    /// Wrong answers (at most a few per thread are reported).
    pub failures: Vec<Failure>,
    /// The highest watched temperature, in °C.
    pub peak_temp_c: Option<f64>,
    /// The temperatures watched and their limits.
    pub watched: Vec<Watched>,
}

/// Reads the watched temperatures and per-CPU clocks; called about twice a
/// second.
pub trait Sensors {
    /// Current value of each watched sensor, by id.
    fn temperatures(&mut self) -> Vec<(String, f64)>;
    /// Current clock of logical CPU `cpu`, in MHz.
    fn clock(&mut self, cpu: u32) -> Option<f64>;
}

/// Ends the worker and reaps it.
fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `worker` (a `surtr worker ...` command) for at most `duration`,
/// stopping it when a [`Watched`] temperature reaches its limit or `stop` is
/// set. `tick` sees each progress event, for a live display.
pub fn run(
    mut worker: Command,
    duration: Duration,
    watched: &[Watched],
    sensors: &mut impl Sensors,
    stop: &AtomicBool,
    mut tick: impl FnMut(&Event, Option<f64>),
) -> Result<Report, String> {
    let mut child = worker
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start the stress worker: {e}"))?;
    let stdout = child.stdout.take().ok_or("no worker output")?;
    let (events, inbox) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(e) = Event::parse(&line)
                && events.send(e).is_err()
            {
                break;
            }
        }
    });

    let start = Instant::now();
    // The worker stops itself at `duration`; give it time to say so.
    let deadline = duration + Duration::from_secs(10);
    let mut report = Report {
        outcome: Outcome::Passed,
        threads: 0,
        duration_ms: 0,
        iterations: Vec::new(),
        failures: Vec::new(),
        peak_temp_c: None,
        watched: watched.to_vec(),
    };
    let mut ended: Option<Outcome> = None;
    let mut finished = false;
    let mut last_temp = None;
    while ended.is_none() && !finished {
        match inbox.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                match &event {
                    Event::Started { threads, .. } => report.threads = *threads,
                    Event::Progress { iterations, .. } => report.iterations.clone_from(iterations),
                    Event::Error {
                        thread,
                        cpu,
                        kernel,
                        elapsed_ms,
                    } => report.failures.push(Failure {
                        thread: *thread,
                        cpu: *cpu,
                        kernel: kernel.clone(),
                        elapsed_ms: *elapsed_ms,
                        clock_mhz: cpu.and_then(|c| sensors.clock(c)),
                    }),
                    Event::Finished { iterations } => {
                        report.iterations.clone_from(iterations);
                        finished = true;
                    }
                }
                tick(&event, last_temp);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Output closed without a finish: see how the process ended.
                let status = child.wait().map_err(|e| e.to_string())?;
                ended = Some(if stop.load(Ordering::Relaxed) {
                    Outcome::Stopped
                } else {
                    Outcome::WorkerFailed {
                        reason: format!("the worker ended early ({status})"),
                    }
                });
                break;
            }
        }
        if stop.load(Ordering::Relaxed) {
            ended = Some(Outcome::Stopped);
        } else if start.elapsed() > deadline {
            ended = Some(Outcome::WorkerFailed {
                reason: "the worker stopped answering".to_owned(),
            });
        }
        for (id, value) in sensors.temperatures() {
            let Some(w) = watched.iter().find(|w| w.id == id) else {
                continue;
            };
            last_temp = Some(last_temp.map_or(value, |t: f64| t.max(value)));
            report.peak_temp_c = Some(report.peak_temp_c.map_or(value, |p: f64| p.max(value)));
            if value >= w.stop_at && ended.is_none() {
                ended = Some(Outcome::TemperatureLimit {
                    sensor: w.label.clone(),
                    value,
                    limit: w.stop_at,
                });
            }
        }
    }
    kill(&mut child);
    report.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    report.outcome = if report.failures.is_empty() {
        ended.unwrap_or(Outcome::Passed)
    } else {
        Outcome::ErrorsFound
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use surtr_model::{Limits, SensorId, Source, Unit};

    use super::*;

    fn temp(device: &str, critical: Option<f64>) -> SensorDescriptor {
        SensorDescriptor {
            id: SensorId::new(format!("{device}/temp")),
            kind: SensorKind::Temperature,
            unit: Unit::Celsius,
            device: device.to_owned(),
            label: "Tctl".to_owned(),
            source: Source::new("test", "test"),
            limits: Limits {
                high: None,
                critical,
            },
        }
    }

    #[test]
    fn limits_come_from_settings_or_the_cpu() {
        let w = watched(
            &[
                temp("cpu0", None),
                temp("cpu1", Some(95.0)),
                temp("cpu2", Some(105.0)),
                temp("nvme", None),
            ],
            DEFAULT_MAX_TEMP_C,
        );
        let stops: Vec<f64> = w.iter().map(|w| w.stop_at).collect();
        // The NVMe drive isn't watched; a CPU with TjMax 95 stops at 90.
        assert_eq!(stops, [95.0, 90.0, 95.0]);
    }
}
