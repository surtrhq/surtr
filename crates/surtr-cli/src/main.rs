//! The `surtr` command.

use std::{error::Error, path::PathBuf, process::ExitCode, thread, time::Duration};

use clap::{Parser, Subcommand};
use serde_json::json;
use surtr_backend::{Provider, ProviderError, mock::MockProvider};
use surtr_backend_amd::AmdProvider;
use surtr_backend_native::{NativeProvider, process_name};
use surtr_backend_nvidia::NvidiaProvider;
use surtr_core::Hub;
use surtr_model::{Feature, SensorId, Timestamp};
use surtr_store::Store;

mod history;
mod render;

/// Hardware inspection, monitoring and stress testing.
#[derive(Debug, Parser)]
#[command(name = "surtr", version)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

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
    /// Show what Surtr can and cannot read on this machine.
    Caps,
    /// Read every sensor once and show the values.
    Sensors {
        /// Milliseconds between the priming read and the reported one; rate
        /// sensors such as CPU load need two samples.
        #[arg(long, default_value_t = 1000)]
        interval: u64,
    },
    /// GPU processes and throttle reasons.
    Gpu {
        #[command(subcommand)]
        command: GpuCommand,
    },
    /// Record every sensor into the history database until stopped (Ctrl+C)
    /// or until --count samples are stored.
    Record {
        /// Milliseconds between samples.
        #[arg(long, default_value_t = 1000)]
        interval: u64,
        /// Stop after this many samples.
        #[arg(long)]
        count: Option<u64>,
        /// Delete samples older than this (for example 7d or 12h); 0 keeps all.
        #[arg(long, default_value = "7d")]
        retain: String,
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

fn db_path(db: Option<&PathBuf>) -> Result<PathBuf, String> {
    db.cloned()
        .or_else(surtr_store::default_path)
        .ok_or_else(|| "no per-user data directory found; pass --db".to_owned())
}

#[derive(Debug, Subcommand)]
enum GpuCommand {
    /// Processes using each GPU, with the GPU memory they hold.
    Procs,
    /// Why each GPU runs below its potential clock right now.
    Throttle,
}

/// Features only GPU backends provide.
const GPU_FEATURES: [Feature; 5] = [
    Feature::GpuIdentity,
    Feature::GpuSensors,
    Feature::GpuProcesses,
    Feature::GpuThrottleReasons,
    Feature::GpuTuning,
];

fn reason(err: ProviderError) -> String {
    match err {
        ProviderError::Unavailable { reason } => reason,
        other => other.to_string(),
    }
}

/// The providers for this run: the mock alone, or the OS backend plus every
/// GPU backend that can start here.
fn hub(mock: bool) -> Result<Hub, String> {
    let mut hub = Hub::new();
    if mock {
        hub.add(Box::new(MockProvider::desktop()));
        return Ok(hub);
    }
    hub.add(Box::new(NativeProvider::new().map_err(reason)?));
    type Started = Result<Box<dyn Provider>, ProviderError>;
    let gpus: [(&str, Started); 2] = [
        ("NVIDIA", NvidiaProvider::new().map(|p| Box::new(p) as _)),
        ("AMD", AmdProvider::new().map(|p| Box::new(p) as _)),
    ];
    let mut why_not = Vec::new();
    for (vendor, result) in gpus {
        match result {
            Ok(provider) => hub.add(provider),
            Err(err) => why_not.push(format!("{vendor}: {}", reason(err))),
        }
    }
    if why_not.len() == 2 {
        hub.add_missing(
            &GPU_FEATURES,
            format!("no supported GPU found ({})", why_not.join("; ")),
        );
    }
    Ok(hub)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = hub(cli.mock).and_then(|hub| run(&cli, &hub).map_err(|e| e.to_string()));
    match result {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("surtr: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli, hub: &Hub) -> Result<String, Box<dyn Error>> {
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
        Command::Caps => {
            let capabilities = hub.capabilities();
            out(
                json!({ "providers": providers, "capabilities": capabilities }),
                render::caps(&names, &capabilities),
            )
        }
        Command::Sensors { interval } => {
            hub.read();
            thread::sleep(Duration::from_millis(*interval));
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
        Command::Record {
            interval,
            count,
            retain,
            db,
        } => {
            let path = db_path(db.as_ref())?;
            let retain = history::parse_duration_ms(retain)?;
            let mut store = Store::open(&path)?;
            let sensors = hub.sensors().len();
            eprintln!(
                "Recording {sensors} sensors to {} every {interval} ms{}.",
                path.display(),
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
                thread::sleep(Duration::from_millis(*interval));
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
        Command::History {
            sensor,
            provider,
            since,
            bucket,
            db,
        } => {
            let path = db_path(db.as_ref())?;
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
