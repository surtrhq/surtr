//! The `surtr` command.

use std::{process::ExitCode, thread, time::Duration};

use clap::{Parser, Subcommand};
use serde::Serialize;
use surtr_backend::{Provider, mock::MockProvider};
use surtr_backend_native::NativeProvider;
use surtr_model::{Capabilities, SensorDescriptor, SystemInfo, Timestamp};

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
    /// Show OS, CPU and memory information (the default).
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
}

/// One sensor with its latest value; `value` is absent if it couldn't be read.
#[derive(Serialize)]
pub struct SensorValue<'a> {
    #[serde(flatten)]
    descriptor: &'a SensorDescriptor,
    value: Option<f64>,
    at: Option<Timestamp>,
}

#[derive(Serialize)]
struct SensorsOutput<'a> {
    provider: &'a str,
    sensors: &'a [SensorValue<'a>],
}

#[derive(Serialize)]
struct InfoOutput<'a> {
    provider: &'a str,
    system: &'a SystemInfo,
}

#[derive(Serialize)]
struct CapsOutput<'a> {
    provider: &'a str,
    capabilities: &'a Capabilities,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let provider: Box<dyn Provider> = if cli.mock {
        Box::new(MockProvider::desktop())
    } else {
        match NativeProvider::new() {
            Ok(p) => Box::new(p),
            Err(err) => {
                eprintln!("surtr: {err}");
                return ExitCode::FAILURE;
            }
        }
    };
    match run(&cli, provider.as_ref()) {
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

fn run(cli: &Cli, provider: &dyn Provider) -> Result<String, Box<dyn std::error::Error>> {
    let id = provider.id();
    Ok(match cli.command.as_ref().unwrap_or(&Command::Info) {
        Command::Info => {
            let system = provider.system_info()?;
            if cli.json {
                json(&InfoOutput {
                    provider: id,
                    system: &system,
                })?
            } else {
                render::info(id, &system)
            }
        }
        Command::Caps => {
            let capabilities = provider.capabilities();
            if cli.json {
                json(&CapsOutput {
                    provider: id,
                    capabilities: &capabilities,
                })?
            } else {
                render::caps(id, &capabilities)
            }
        }
        Command::Sensors { interval } => {
            let descriptors = provider.sensors()?;
            provider.read_sensors()?;
            thread::sleep(Duration::from_millis(*interval));
            let readings = provider.read_sensors()?;
            let sensors: Vec<SensorValue> = descriptors
                .iter()
                .map(|descriptor| {
                    let r = readings.iter().find(|r| r.id == descriptor.id);
                    SensorValue {
                        descriptor,
                        value: r.map(|r| r.value),
                        at: r.map(|r| r.at),
                    }
                })
                .collect();
            if cli.json {
                json(&SensorsOutput {
                    provider: id,
                    sensors: &sensors,
                })?
            } else {
                render::sensors(id, &sensors)
            }
        }
    })
}

fn json(value: &impl Serialize) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value).map(|s| s + "\n")
}
