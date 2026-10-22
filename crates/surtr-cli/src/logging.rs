//! Log files and the panic hook (plan, section 8.1).
//!
//! Logs go to daily files in the per-user log directory, never to the
//! terminal, and the last 7 are kept. `SURTR_LOG` sets the level (for example
//! `debug` or `surtr_core=trace`); the default is `info`. A panic writes a
//! crash file with the message and a backtrace before anything else happens.

use std::{backtrace::Backtrace, fs, panic, path::PathBuf};

use surtr_core::paths;
use surtr_model::Timestamp;
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::EnvFilter;

/// Keeps the log writer flushing until dropped at the end of `main`.
pub struct Logging {
    _guard: Option<WorkerGuard>,
}

/// Starts logging and installs the panic hook. Failing to set up logging
/// never stops Surtr; it just runs without log files.
pub fn init() -> Logging {
    let Some(dir) = paths::log_dir() else {
        return Logging { _guard: None };
    };
    install_panic_hook(dir.clone());
    if fs::create_dir_all(&dir).is_err() {
        return Logging { _guard: None };
    }
    let Ok(appender) = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("surtr")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
    else {
        return Logging { _guard: None };
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("SURTR_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init();
    Logging {
        _guard: Some(guard),
    }
}

fn install_panic_hook(dir: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let text = format!(
            "Surtr {} ({} {}) panicked: {info}\n\nBacktrace:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            Backtrace::force_capture()
        );
        let text = paths::redact(&text);
        if fs::create_dir_all(&dir).is_ok() {
            let _ = fs::write(dir.join(format!("crash-{}.txt", Timestamp::now().0)), &text);
        }
        tracing::error!("{}", text.lines().next().unwrap_or("panic"));
        previous(info);
    }));
}
