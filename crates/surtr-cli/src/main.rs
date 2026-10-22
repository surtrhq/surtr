//! The `surtr` command.

use std::{
    cell::Cell, error::Error, io::IsTerminal, path::PathBuf, process::ExitCode, thread,
    time::Duration,
};

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::json;
use surtr_backend::{Provider, ProviderError, mock::MockProvider};
use surtr_backend_amd::AmdProvider;
use surtr_backend_native::{AMD, INTEL, NVIDIA, NativeProvider, process_name};
use surtr_backend_nvidia::NvidiaProvider;
use surtr_core::{
    Hub,
    config::{self, Config},
};
use surtr_model::{Feature, Remedy, SensorId, Timestamp};
use surtr_store::Store;

mod diag;
mod export;
mod history;
mod logging;
mod notify;
mod remote;
mod render;
mod serve;
mod style;
mod watch;

const EXAMPLES: &str = "\
Examples:
  surtr                          OS, CPU, memory and GPUs
  surtr tui                      interactive terminal UI (works over SSH)
  surtr status                   health summary (exit code 0 ok, 2 warning, 3 critical)
  surtr sensors                  every sensor once
  surtr watch temp gpu           live values with trends, filtered
  surtr sensors --json           the same as JSON, with each value's source
  surtr gpu throttle             why each GPU is below its potential clock
  surtr record --count 60        one minute of history
  surtr serve --metrics 127.0.0.1:9469   record continuously, serve Prometheus metrics
  surtr history cpu/load --since 10m --bucket 1m
  surtr export --since 1h --bucket 10s > history.csv
  surtr diag                     bug-report bundle (preview first, nothing uploaded)
  surtr --mock info              the built-in test machine

Colors turn off when output is piped or NO_COLOR is set.";

/// Hardware inspection, monitoring and stress testing.
#[derive(Debug, Parser)]
#[command(name = "surtr", version, after_help = EXAMPLES)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Read the hardware directly even if `surtr serve` is running (by default
    /// commands use a running daemon's latest readings).
    #[arg(long, global = true)]
    embedded: bool,

    /// Use the built-in mock provider instead of this machine's hardware.
    #[arg(long, global = true)]
    mock: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show OS, CPU, memory and GPU information (the default).
    Info,
    /// Show a health summary with plain-language findings. Exit code: 0 ok,
    /// 2 warning, 3 critical (1 is an error running surtr).
    Status {
        /// Milliseconds between the priming read and the reported one
        /// (default: general.interval_ms in the settings, else 1000).
        #[arg(long)]
        interval: Option<u64>,
    },
    /// Show, locate or create the settings file.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print a shell completion script, for example
    /// `surtr completions powershell >> $PROFILE`.
    Completions {
        /// The shell.
        shell: clap_complete::Shell,
    },
    /// Show what Surtr can and cannot read on this machine.
    Caps,
    /// Read every sensor once and show the values.
    Sensors {
        /// Milliseconds between the priming read and the reported one; rate
        /// sensors such as CPU load need two samples (default: settings, else
        /// 1000).
        #[arg(long)]
        interval: Option<u64>,
    },
    /// Create a diagnostic bundle for a bug report: a Markdown summary plus
    /// redacted system info, capabilities, sensors and recent logs, as a zip.
    /// Shows what it contains and asks before writing; nothing is uploaded.
    Diag {
        /// Only print the Markdown summary (to paste into an issue).
        #[arg(long)]
        summary: bool,
        /// Where to write the zip (default: surtr-diag-<time>.zip here).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write without asking.
        #[arg(long, short)]
        yes: bool,
    },
    /// Live table of sensors with trends, redrawn in place (Ctrl+C to stop).
    /// With --json, one JSON object per line for scripts.
    Watch {
        /// Only sensors whose id, label or device contains one of these
        /// (for example `temp`, `gpu`, `cpu/load`). Default: all but
        /// per-thread sensors.
        filters: Vec<String>,
        /// Milliseconds between updates (default: settings, else 1000).
        #[arg(long)]
        interval: Option<u64>,
        /// Stop after this many updates.
        #[arg(long)]
        count: Option<u64>,
    },
    /// Interactive terminal UI: overview, sensors with trends, GPUs and
    /// capabilities. Works over SSH. Press ? inside for keys.
    Tui {
        /// Milliseconds between refreshes (default: settings, else 1000).
        #[arg(long)]
        interval: Option<u64>,
    },
    /// GPU processes and throttle reasons.
    Gpu {
        #[command(subcommand)]
        command: GpuCommand,
    },
    /// Show recent alerts that `surtr serve` raised, or with --rules the
    /// rules it uses.
    Alerts {
        /// Show the alert rules for this machine's sensors instead.
        #[arg(long)]
        rules: bool,
        /// How many recent alerts to show.
        #[arg(long, default_value_t = 20)]
        last: usize,
        /// Database file whose alerts to show (default: settings, else the
        /// per-user data directory).
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Run in the background: poll sensors, record history and, with
    /// --metrics, serve Prometheus metrics. Stop with Ctrl+C.
    Serve {
        /// Milliseconds between reads (default: settings, else 1000).
        #[arg(long)]
        interval: Option<u64>,
        /// Stop after this many reads.
        #[arg(long)]
        count: Option<u64>,
        /// Delete history older than this (default: settings, else 7d).
        #[arg(long)]
        retain: Option<String>,
        /// Database file (default: settings, else the per-user data directory).
        #[arg(long)]
        db: Option<PathBuf>,
        /// Serve Prometheus metrics at this address, for example
        /// 127.0.0.1:9469 (port 0 picks a free one).
        #[arg(long)]
        metrics: Option<std::net::SocketAddr>,
        /// Allow a --metrics address reachable from other machines. There
        /// is no authentication.
        #[arg(long)]
        metrics_remote: bool,
        /// No desktop notifications for alerts (overrides alerts.notify).
        #[arg(long)]
        no_notify: bool,
    },
    /// Record every sensor into the history database until stopped (Ctrl+C)
    /// or until --count samples are stored.
    Record {
        /// Milliseconds between samples (default: settings, else 1000).
        #[arg(long)]
        interval: Option<u64>,
        /// Stop after this many samples.
        #[arg(long)]
        count: Option<u64>,
        /// Delete samples older than this (for example 7d or 12h); 0 keeps all
        /// (default: history.retain in the settings, else 7d).
        #[arg(long)]
        retain: Option<String>,
        /// Database file (default: the per-user data directory).
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Export recorded history as CSV (default) or JSON, one row per sensor
    /// per time bucket.
    Export {
        /// Only sensors whose id, label, device or provider contains one of
        /// these. Default: all recorded sensors.
        filters: Vec<String>,
        /// How far back to export (for example 30m, 2h, 7d).
        #[arg(long, default_value = "1h")]
        since: String,
        /// Bucket length (for example 10s, 1m); default: every sample.
        #[arg(long)]
        bucket: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Csv)]
        format: ExportFormat,
        /// Write to this file instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Database file (default: the per-user data directory).
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// List recorded sensors, or show the stored history of one sensor.
    History {
        /// Sensor id as shown by `surtr history` (for example cpu/load).
        sensor: Option<String>,
        /// Provider of the sensor, if several have one with this id.
        #[arg(long)]
        provider: Option<String>,
        /// How far back to look (for example 30s, 10m, 2h, 1d).
        #[arg(long, default_value = "1h")]
        since: String,
        /// Group samples into buckets of this length (for example 1m),
        /// showing average, minimum and maximum.
        #[arg(long)]
        bucket: Option<String>,
        /// Database file (default: the per-user data directory).
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

fn db_path(db: Option<&PathBuf>, settings: &Config) -> Result<PathBuf, String> {
    db.cloned()
        .or_else(|| settings.history.path.as_ref().map(PathBuf::from))
        .or_else(surtr_core::paths::history_db)
        .ok_or_else(|| "no per-user data directory found; pass --db".to_owned())
}

/// The settings file, or defaults. Problems are reported on stderr and
/// never stop Surtr.
fn load_settings() -> Config {
    let Some(path) = surtr_core::paths::config_file() else {
        return Config::default();
    };
    match config::load(&path) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                eprintln!(
                    "surtr: {}: {w}",
                    surtr_core::paths::redact(&path.display().to_string())
                );
            }
            loaded.config
        }
        Err(err) => {
            eprintln!(
                "surtr: ignoring the settings file: {}",
                surtr_core::paths::redact(&err)
            );
            Config::default()
        }
    }
}

/// `surtr config show|path|init`.
fn config_command(
    command: &ConfigCommand,
    settings: &Config,
    json: bool,
) -> Result<String, String> {
    let path = surtr_core::paths::config_file()
        .ok_or_else(|| "no per-user configuration directory found".to_owned())?;
    let shown = surtr_core::paths::redact(&path.display().to_string());
    match command {
        ConfigCommand::Path => Ok(format!("{shown}\n")),
        ConfigCommand::Show if json => serde_json::to_string_pretty(settings)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string()),
        ConfigCommand::Show => Ok(format!("# {shown}\n{}", settings.effective())),
        ConfigCommand::Init => {
            if path.exists() {
                return Err(format!(
                    "{shown} already exists; edit it or remove it first"
                ));
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, config::TEMPLATE).map_err(|e| e.to_string())?;
            Ok(format!("Wrote {shown} with every setting commented out.\n"))
        }
    }
}

#[derive(Debug, Subcommand)]
enum GpuCommand {
    /// Processes using each GPU, with the GPU memory they hold.
    Procs,
    /// Why each GPU runs below its potential clock right now.
    Throttle,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the effective settings (defaults filled in).
    Show,
    /// Print where the settings file is.
    Path,
    /// Create the settings file with every setting commented out.
    Init,
}

/// File formats for `surtr export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ExportFormat {
    /// Comma-separated values with a header row.
    Csv,
    /// A JSON array of series.
    Json,
}

/// Features only GPU backends provide.
const GPU_FEATURES: [Feature; 5] = [
    Feature::GpuIdentity,
    Feature::GpuSensors,
    Feature::GpuProcesses,
    Feature::GpuThrottleReasons,
    Feature::GpuTuning,
];

/// How to get a GPU backend working when its GPU is present. Only official
/// vendor pages; Surtr never installs drivers itself (#24).
fn gpu_remedy(vendor: u16) -> Option<Remedy> {
    match vendor {
        NVIDIA => Some(
            Remedy::new("Install or repair the NVIDIA driver; NVML comes with it")
                .url("https://www.nvidia.com/drivers"),
        ),
        AMD if cfg!(target_os = "linux") => {
            Some(Remedy::new("Load the amdgpu kernel driver").command("sudo modprobe amdgpu"))
        }
        // AMD on Windows needs the ADLX backend, which doesn't exist yet.
        _ => None,
    }
}

fn reason(err: ProviderError) -> String {
    match err {
        ProviderError::Unavailable { reason } => reason,
        other => other.to_string(),
    }
}

/// The providers for this run: the mock alone; a running `surtr serve` (unless
/// `embedded`); or the OS backend plus every GPU backend that can start here.
fn hub(mock: bool, embedded: bool) -> Result<Hub, String> {
    let mut hub = Hub::new();
    if mock {
        hub.add(Box::new(MockProvider::desktop()));
        return Ok(hub);
    }
    if !embedded && let Some(daemon) = remote::RemoteProvider::connect() {
        tracing::info!("using surtr serve over local IPC");
        hub.add(Box::new(daemon));
        return Ok(hub);
    }
    hub.add(Box::new(NativeProvider::new().map_err(reason)?));
    // Which GPUs the OS sees decides how to explain a backend that can't
    // start: a missing driver for a GPU that is there, or no GPU of that kind.
    let adapters = surtr_backend_native::display_adapters();
    let present = |vendor: u16| adapters.iter().any(|a| a.vendor_id == vendor);
    type Started = Result<Box<dyn Provider>, ProviderError>;
    let gpus: [(&str, u16, Started); 2] = [
        (
            "NVIDIA",
            NVIDIA,
            NvidiaProvider::new().map(|p| Box::new(p) as _),
        ),
        ("AMD", AMD, AmdProvider::new().map(|p| Box::new(p) as _)),
    ];
    let mut started = 0;
    let mut explained = false;
    let mut why_not = Vec::new();
    for (vendor, id, result) in gpus {
        match result {
            Ok(provider) => {
                hub.add(provider);
                started += 1;
            }
            Err(err) if present(id) => {
                hub.add_missing(
                    &GPU_FEATURES,
                    format!("{vendor} GPU found, but {}", reason(err)),
                    gpu_remedy(id),
                );
                explained = true;
            }
            Err(err) => why_not.push(format!("{vendor}: {}", reason(err))),
        }
    }
    if started == 0 && !explained {
        let text = if present(INTEL) {
            "Intel GPUs have no Surtr backend yet".to_owned()
        } else {
            format!("no supported GPU found ({})", why_not.join("; "))
        };
        hub.add_missing(&GPU_FEATURES, text, None);
    }
    Ok(hub)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Completions need no hardware access.
    if let Some(Command::Completions { shell }) = &cli.command {
        clap_complete::generate(*shell, &mut Cli::command(), "surtr", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    let settings = load_settings();
    // Settings commands need no hardware access either.
    if let Some(Command::Config { command }) = &cli.command {
        return match config_command(command, &settings, cli.json) {
            Ok(text) => {
                anstream::print!("{text}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("surtr: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let _logging = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), command = ?cli.command, "surtr started");
    let code = Cell::new(0);
    // The daemon and the recorder read the hardware themselves.
    let embedded = cli.embedded
        || matches!(
            cli.command,
            Some(Command::Serve { .. } | Command::Record { .. })
        );
    let result = hub(cli.mock, embedded)
        .and_then(|hub| run(&cli, &hub, &code, &settings).map_err(|e| e.to_string()));
    match result {
        Ok(output) => {
            // anstream drops styling when not writing to a terminal.
            anstream::print!("{output}");
            ExitCode::from(code.get())
        }
        Err(err) => {
            eprintln!("surtr: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the command and returns its output; `code` receives a non-zero exit
/// code for commands that report one (`status`).
fn run(cli: &Cli, hub: &Hub, code: &Cell<u8>, settings: &Config) -> Result<String, Box<dyn Error>> {
    // A flag wins over the settings file, which wins over the default.
    let ms = |flag: &Option<u64>| flag.or(settings.general.interval_ms).unwrap_or(1000);
    let providers = hub.provider_ids();
    let names = providers.join(", ");
    let out = |value: serde_json::Value, text: String| -> Result<String, Box<dyn Error>> {
        if cli.json {
            Ok(serde_json::to_string_pretty(&value)? + "\n")
        } else {
            Ok(text)
        }
    };
    match cli.command.as_ref().unwrap_or(&Command::Info) {
        Command::Info => {
            let system = hub.system_info();
            out(
                json!({ "providers": providers, "system": system }),
                render::info(&names, &system),
            )
        }
        Command::Status { interval } => {
            hub.read();
            thread::sleep(Duration::from_millis(ms(interval)));
            let samples = hub.read();
            let status = surtr_view::status(
                &hub.system_info(),
                &samples,
                &hub.throttle_reasons(),
                &hub.capabilities(),
            );
            code.set(match status.level {
                surtr_view::Level::Ok => 0,
                surtr_view::Level::Warn => 2,
                surtr_view::Level::Critical => 3,
            });
            let findings: Vec<_> = status
                .findings
                .iter()
                .map(|f| json!({ "level": f.level.map(|l| l.label()), "message": f.message }))
                .collect();
            let parts: Vec<_> = status
                .parts
                .iter()
                .map(|r| json!({ "part": r.label, "summary": r.cells[0].text }))
                .collect();
            out(
                json!({
                    "providers": providers,
                    "level": status.level.label(),
                    "parts": parts,
                    "findings": findings,
                }),
                render::status(&status),
            )
        }
        Command::Completions { .. } | Command::Config { .. } => {
            unreachable!("handled before hardware access")
        }
        Command::Diag {
            summary,
            out: path,
            yes,
        } => {
            let bundle = diag::build(hub);
            if *summary {
                return out(json!({ "summary": bundle.summary }), bundle.summary.clone());
            }
            let path = path.clone().unwrap_or_else(|| {
                PathBuf::from(format!("surtr-diag-{}.zip", diag::stamp(Timestamp::now())))
            });
            let shown = surtr_core::paths::redact(&path.display().to_string());
            if !cli.json {
                anstream::println!("{}", bundle.summary);
                anstream::println!("The bundle will contain:");
                for (name, bytes) in &bundle.files {
                    anstream::println!("  {name:<40}{:>9} bytes", bytes.len());
                }
                anstream::println!();
            }
            if !yes {
                if !std::io::stdin().is_terminal() {
                    return Err("pass --yes to write the bundle without a prompt".into());
                }
                anstream::print!("Write {shown}? [y/N] ");
                std::io::Write::flush(&mut anstream::stdout())?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "y" | "Y" | "yes") {
                    return Ok("Nothing written.\n".to_owned());
                }
            }
            diag::write(&bundle, &path)?;
            let names: Vec<&str> = bundle.files.iter().map(|(n, _)| n.as_str()).collect();
            out(
                json!({ "path": shown, "files": names }),
                format!(
                    "Wrote {shown}. Attach it to your issue and paste summary.md into the description; nothing was uploaded.\n"
                ),
            )
        }
        Command::Watch {
            filters,
            interval,
            count,
        } => {
            watch::run(
                hub,
                filters,
                Duration::from_millis(ms(interval)),
                *count,
                cli.json,
            )?;
            Ok(String::new())
        }
        Command::Tui { interval } => {
            let interval = Duration::from_millis(ms(interval).max(100));
            let theme = match settings.general.theme.as_deref() {
                Some("light") => surtr_view::Theme::LIGHT,
                _ => surtr_view::Theme::DARK,
            };
            surtr_tui::run(hub, interval, theme, &process_name)?;
            Ok(String::new())
        }
        Command::Caps => {
            let capabilities = hub.capabilities();
            out(
                json!({ "providers": providers, "capabilities": capabilities }),
                render::caps(&names, &capabilities),
            )
        }
        Command::Sensors { interval } => {
            hub.read();
            thread::sleep(Duration::from_millis(ms(interval)));
            let samples = hub.read();
            out(
                json!({ "providers": providers, "sensors": samples }),
                render::sensors(&names, &samples),
            )
        }
        Command::Gpu {
            command: GpuCommand::Procs,
        } => {
            let mut processes = hub.gpu_processes();
            for p in processes.iter_mut().filter(|p| p.name.is_none()) {
                p.name = process_name(p.pid);
            }
            out(
                json!({ "providers": providers, "processes": processes }),
                render::gpu_processes(&processes),
            )
        }
        Command::Gpu {
            command: GpuCommand::Throttle,
        } => {
            let throttle = hub.throttle_reasons();
            out(
                json!({ "providers": providers, "throttle": throttle }),
                render::throttle(&throttle),
            )
        }
        Command::Alerts { rules, last, db } => {
            if *rules {
                let rules = surtr_view::health::default_alert_rules(&hub.sensors());
                let mut text = String::new();
                for r in &rules {
                    let (op, t) = match r.condition {
                        surtr_core::alerts::Condition::Above(t) => ("≥", t),
                        surtr_core::alerts::Condition::Below(t) => ("≤", t),
                    };
                    text.push_str(&format!(
                        "{:<9}{op} {t:<6} for {:>3} s  {}\n",
                        format!("{:?}", r.severity).to_lowercase(),
                        r.for_ms / 1000,
                        r.name
                    ));
                }
                if rules.is_empty() {
                    text.push_str(
                        "No alert rules: this machine has no temperature sensors Surtr can read.\n",
                    );
                }
                return out(json!({ "rules": rules }), text);
            }
            let path = serve::alerts_path(&db_path(db.as_ref(), settings)?);
            let events: Vec<surtr_core::alerts::Event> = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let recent = &events[events.len().saturating_sub(*last)..];
            let mut text = String::new();
            if recent.is_empty() {
                text.push_str("No alerts recorded. `surtr serve` raises and records them.\n");
            }
            for e in recent {
                text.push_str(&format!(
                    "{}  {}\n",
                    history::iso_utc(e.at),
                    serve::describe(e)
                ));
            }
            out(json!({ "alerts": recent }), text)
        }
        Command::Serve {
            interval,
            count,
            retain,
            db,
            metrics,
            metrics_remote,
            no_notify,
        } => {
            let retain_ms = history::parse_duration_ms(
                retain
                    .as_deref()
                    .or(settings.history.retain.as_deref())
                    .unwrap_or("7d"),
            )?;
            serve::run(
                hub,
                &serve::Options {
                    interval: Duration::from_millis(ms(interval)),
                    count: *count,
                    retain_ms,
                    db: db_path(db.as_ref(), settings)?,
                    metrics: *metrics,
                    metrics_remote: *metrics_remote,
                    notify: settings.alerts.notify() && !*no_notify,
                },
            )
        }
        Command::Record {
            interval,
            count,
            retain,
            db,
        } => {
            let path = db_path(db.as_ref(), settings)?;
            let retain = history::parse_duration_ms(
                retain
                    .as_deref()
                    .or(settings.history.retain.as_deref())
                    .unwrap_or("7d"),
            )?;
            let interval = &Some(ms(interval));
            let mut store = Store::open(&path)?;
            let sensors = hub.sensors().len();
            eprintln!(
                "Recording {sensors} sensors to {} every {} ms{}.",
                path.display(),
                ms(interval),
                if count.is_none() {
                    "; press Ctrl+C to stop"
                } else {
                    ""
                }
            );
            // Rate sensors need a first sample to compare with.
            hub.read();
            let mut stored = 0;
            let mut taken = 0;
            while count.is_none_or(|c| taken < c) {
                thread::sleep(Duration::from_millis(ms(interval)));
                let samples = hub.read();
                stored += store.record(
                    samples
                        .iter()
                        .filter_map(|s| Some((&s.descriptor, s.at?, s.value?))),
                )?;
                taken += 1;
                // Prune now and then, not on every sample.
                if retain > 0 && taken % 600 == 1 {
                    store.prune(Timestamp(Timestamp::now().0.saturating_sub(retain)))?;
                }
            }
            out(
                json!({ "db": path, "samples": taken, "values": stored }),
                format!("Stored {stored} values from {taken} samples.\n"),
            )
        }
        Command::Export {
            filters,
            since,
            bucket,
            format,
            out: file,
            db,
        } => {
            let path = db_path(db.as_ref(), settings)?;
            let store = Store::open(&path)?;
            let sensors = store.sensors()?;
            let now = Timestamp::now();
            let from = Timestamp(now.0.saturating_sub(history::parse_duration_ms(since)?));
            let bucket_ms = match bucket {
                Some(b) => history::parse_duration_ms(b)?.max(1),
                None => 1,
            };
            let series = export::collect(
                &store,
                &sensors,
                filters,
                from,
                Timestamp(now.0 + 1),
                bucket_ms,
            )?;
            if series.is_empty() {
                return Err(format!("no recorded sensor matches in {}", path.display()).into());
            }
            let text = match format {
                ExportFormat::Csv => export::csv(&series),
                ExportFormat::Json => {
                    serde_json::to_string_pretty(&export::json(&series, bucket_ms))? + "\n"
                }
            };
            match file {
                Some(file) => {
                    std::fs::write(file, &text)?;
                    let rows: usize = series.iter().map(|s| s.buckets.len()).sum();
                    Ok(format!(
                        "Wrote {rows} rows for {} sensors to {}.\n",
                        series.len(),
                        surtr_core::paths::redact(&file.display().to_string())
                    ))
                }
                None => Ok(text),
            }
        }
        Command::History {
            sensor,
            provider,
            since,
            bucket,
            db,
        } => {
            let path = db_path(db.as_ref(), settings)?;
            let store = Store::open(&path)?;
            let known = store.sensors()?;
            let Some(sensor) = sensor else {
                return out(
                    json!({ "db": path, "sensors": history::sensors_json(&known) }),
                    history::sensors(&path, &known),
                );
            };
            let matches: Vec<_> = known
                .iter()
                .filter(|s| s.descriptor.id.0 == *sensor)
                .filter(|s| {
                    provider
                        .as_ref()
                        .is_none_or(|p| s.descriptor.source.provider == *p)
                })
                .collect();
            let stored = match matches.as_slice() {
                [one] => *one,
                [] => {
                    return Err(
                        format!("no recorded sensor {sensor:?} in {}", path.display()).into(),
                    );
                }
                _ => {
                    return Err(format!(
                        "several providers have {sensor:?}; pass --provider ({})",
                        matches
                            .iter()
                            .map(|s| s.descriptor.source.provider.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .into());
                }
            };
            let now = Timestamp::now();
            let from = Timestamp(now.0.saturating_sub(history::parse_duration_ms(since)?));
            let bucket_ms = match bucket {
                Some(b) => history::parse_duration_ms(b)?.max(1),
                None => 1,
            };
            let buckets = store.history(
                &stored.descriptor.source.provider,
                &SensorId(sensor.clone()),
                from,
                Timestamp(now.0 + 1),
                bucket_ms,
            )?;
            out(
                json!({
                    "db": path,
                    "sensor": stored.descriptor,
                    "bucket_ms": bucket_ms,
                    "points": history::buckets_json(&buckets),
                }),
                history::render(&stored.descriptor, &buckets, bucket_ms > 1),
            )
        }
    }
}
