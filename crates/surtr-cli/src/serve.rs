//! `surtr serve`: the background process (plan, sections 4 and 5.6). This
//! first version polls the sensors, records history, and with `--metrics`
//! serves Prometheus metrics. Front ends talking to it over local IPC come
//! next (#33).

use std::{
    error::Error,
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use axum::{Router, http::header, routing::get};
use surtr_core::{Hub, paths};
use surtr_model::Timestamp;
use surtr_store::Store;
use surtr_view::Level;

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
    let metrics = Arc::new(Mutex::new(String::new()));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = match opts.metrics {
        Some(addr) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()?;
            let listener = runtime.block_on(tokio::net::TcpListener::bind(addr))?;
            eprintln!(
                "Serving metrics on http://{}/metrics",
                listener.local_addr()?
            );
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
            Some(thread::spawn(move || {
                runtime.block_on(async move {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await
                })
            }))
        }
        None => None,
    };

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
    let system = hub.system_info();
    let capabilities = hub.capabilities();
    // Rate sensors need a first sample to compare with.
    hub.read();
    let mut reads = 0;
    let mut stored = 0;
    while opts.count.is_none_or(|c| reads < c) {
        thread::sleep(opts.interval);
        let samples = hub.read();
        let throttle = hub.throttle_reasons();
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
        reads += 1;
        // Prune now and then, not on every read.
        if opts.retain_ms > 0 && reads % 600 == 1 {
            store.prune(Timestamp(Timestamp::now().0.saturating_sub(opts.retain_ms)))?;
        }
    }

    let _ = stop.send(());
    if let Some(server) = server {
        let _ = server.join();
    }
    drop(lock);
    Ok(format!("Stored {stored} values from {reads} reads.\n"))
}
