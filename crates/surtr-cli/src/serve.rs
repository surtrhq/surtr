//! `surtr serve`: the background process (plan, sections 4 and 5.6). It polls
//! the sensors, records history, raises alerts, answers front ends over the
//! local IPC endpoint, and with `--metrics` serves Prometheus metrics.

use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use axum::{Router, http::header, routing::get};
use serde_json::{Value, json};
use surtr_backend_native::process_name;
use surtr_core::{Hub, Sample, alerts, paths};
use surtr_model::{GpuProcess, ThrottleState, Timestamp};
use surtr_store::Store;
use surtr_view::Level;
use tokio::sync::watch;

/// What `surtr serve` was asked to do.
pub struct Options {
    /// Time between reads.
    pub interval: Duration,
    /// Stop after this many reads (tests); `None` runs until stopped.
    pub count: Option<u64>,
    /// Delete history older than this many milliseconds; 0 keeps all.
    pub retain_ms: u64,
    /// History database.
    pub db: PathBuf,
    /// Where to serve `/metrics`, if anywhere.
    pub metrics: Option<SocketAddr>,
    /// Allow a non-loopback metrics address.
    pub metrics_remote: bool,
    /// Show desktop notifications for fired alerts.
    pub notify: bool,
}

/// The latest read, which IPC clients get instead of reading the hardware
/// themselves.
#[derive(Debug, Default)]
struct Snapshot {
    samples: Vec<Sample>,
    throttle: Vec<ThrottleState>,
    processes: Vec<GpuProcess>,
}

/// Where `surtr serve` appends alerts: `alerts.jsonl` next to the database.
pub fn alerts_path(db: &Path) -> PathBuf {
    db.with_file_name("alerts.jsonl")
}

/// One line for an alert event, for the log and the terminal.
pub fn describe(e: &alerts::Event) -> String {
    let state = match e.state {
        alerts::State::Fired => "ALERT",
        alerts::State::Resolved => "RESOLVED",
    };
    let severity = match e.severity {
        alerts::Severity::Warning => "warning",
        alerts::Severity::Critical => "critical",
    };
    format!(
        "{state} {severity}: {} {} at {:.1} ({})",
        e.device, e.label, e.value, e.rule
    )
}

/// Answers IPC method calls from the snapshot and the static information.
fn handler(
    snapshot: Arc<Mutex<Snapshot>>,
    system: Value,
    capabilities: Value,
) -> surtr_ipc::Handler {
    Arc::new(move |method: &str| {
        let s = snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        let value = match method {
            "system_info" => return Ok(system.clone()),
            "capabilities" => return Ok(capabilities.clone()),
            "sensors" => json!(s.samples.iter().map(|x| &x.descriptor).collect::<Vec<_>>()),
            "read" => json!(s.samples),
            "gpu_processes" => json!(s.processes),
            "throttle_reasons" => json!(s.throttle),
            other => return Err(format!("unknown method {other}")),
        };
        Ok(value)
    })
}

/// Resolves when `stop` turns true.
async fn stopped(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

/// Runs until stopped (Ctrl+C) or `count` reads.
pub fn run(hub: &Hub, opts: &Options) -> Result<String, Box<dyn Error>> {
    if let Some(addr) = opts.metrics
        && !addr.ip().is_loopback()
    {
        if !opts.metrics_remote {
            return Err(format!(
                "{addr} is reachable from other machines; use a loopback address such as \
                 127.0.0.1, or pass --metrics-remote if you really want that"
            )
            .into());
        }
        eprintln!(
            "surtr: warning: metrics on {addr} have no authentication; anyone who can reach \
             this port can read this machine's hardware details"
        );
    }

    // One serve per database: a second one would double-poll the hardware.
    let lock_path = opts.db.with_extension("lock");
    if let Some(dir) = lock_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = File::create(&lock_path)?;
    if lock.try_lock().is_err() {
        return Err(format!(
            "another surtr serve is already running for {}",
            paths::redact(&opts.db.display().to_string())
        )
        .into());
    }

    let mut store = Store::open(&opts.db)?;
    let system = hub.system_info();
    let capabilities = hub.capabilities();
    // Rate sensors need a first sample to compare with; it also gives IPC
    // clients the sensor list before the first full read.
    let snapshot = Arc::new(Mutex::new(Snapshot {
        samples: hub.read(),
        ..Snapshot::default()
    }));
    let metrics = Arc::new(Mutex::new(String::new()));

    // IPC and metrics share one small runtime on its own thread.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let listener = match opts.metrics {
        Some(addr) => {
            let listener = runtime.block_on(tokio::net::TcpListener::bind(addr))?;
            eprintln!(
                "Serving metrics on http://{}/metrics",
                listener.local_addr()?
            );
            Some(listener)
        }
        None => None,
    };
    let text = Arc::clone(&metrics);
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let body = text.lock().unwrap_or_else(PoisonError::into_inner).clone();
            async move {
                (
                    [(
                        header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    body,
                )
            }
        }),
    );
    let endpoint = surtr_ipc::endpoint()?;
    let hello = surtr_ipc::Hello {
        protocol: surtr_ipc::PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        providers: hub.provider_ids().into_iter().map(str::to_owned).collect(),
    };
    let answer = handler(Arc::clone(&snapshot), json!(system), json!(capabilities));
    let (stop, stop_rx) = watch::channel(false);
    let server = thread::spawn(move || {
        runtime.block_on(async move {
            let ipc = async {
                if let Err(e) =
                    surtr_ipc::serve(&endpoint, hello, answer, stopped(stop_rx.clone())).await
                {
                    eprintln!("surtr: warning: front ends can't connect: {e}");
                }
            };
            let http = async {
                if let Some(listener) = listener
                    && let Err(e) = axum::serve(listener, app)
                        .with_graceful_shutdown(stopped(stop_rx.clone()))
                        .await
                {
                    eprintln!("surtr: warning: metrics server stopped: {e}");
                }
            };
            tokio::join!(ipc, http);
        });
    });

    eprintln!(
        "Recording to {} every {} ms{}.",
        paths::redact(&opts.db.display().to_string()),
        opts.interval.as_millis(),
        if opts.count.is_none() {
            "; press Ctrl+C to stop"
        } else {
            ""
        }
    );
    let mut engine = alerts::Engine::new(surtr_view::health::default_alert_rules(&hub.sensors()));
    let mut alert_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(alerts_path(&opts.db))?;
    let mut reads = 0;
    let mut stored = 0;
    while opts.count.is_none_or(|c| reads < c) {
        thread::sleep(opts.interval);
        let samples = hub.read();
        let throttle = hub.throttle_reasons();
        let mut processes = hub.gpu_processes();
        for p in processes.iter_mut().filter(|p| p.name.is_none()) {
            p.name = process_name(p.pid);
        }
        stored += store.record(
            samples
                .iter()
                .filter_map(|s| Some((&s.descriptor, s.at?, s.value?))),
        )?;
        let level = match surtr_view::status(&system, &samples, &throttle, &capabilities).level {
            Level::Ok => 0,
            Level::Warn => 1,
            Level::Critical => 2,
        };
        *metrics.lock().unwrap_or_else(PoisonError::into_inner) =
            surtr_export::prometheus(&samples, &throttle, Some(level));
        for event in engine.evaluate(&samples, Timestamp::now()) {
            writeln!(alert_log, "{}", serde_json::to_string(&event)?)?;
            let line = describe(&event);
            tracing::warn!("{line}");
            eprintln!("{line}");
            if opts.notify {
                crate::notify::send(&event);
            }
        }
        *snapshot.lock().unwrap_or_else(PoisonError::into_inner) = Snapshot {
            samples,
            throttle,
            processes,
        };
        reads += 1;
        // Prune now and then, not on every read.
        if opts.retain_ms > 0 && reads % 600 == 1 {
            store.prune(Timestamp(Timestamp::now().0.saturating_sub(opts.retain_ms)))?;
        }
    }

    let _ = stop.send(true);
    let _ = server.join();
    drop(lock);
    Ok(format!("Stored {stored} values from {reads} reads.\n"))
}
