//! The `surtr` command.

use std::{process::ExitCode, thread, time::Duration};

use clap::{Parser, Subcommand};
use serde_json::json;
use surtr_backend::{Provider, ProviderError, mock::MockProvider};
use surtr_backend_amd::AmdProvider;
use surtr_backend_native::{NativeProvider, process_name};
use surtr_backend_nvidia::NvidiaProvider;
use surtr_core::Hub;
use surtr_model::Feature;

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

fn run(cli: &Cli, hub: &Hub) -> serde_json::Result<String> {
    let providers = hub.provider_ids();
    let names = providers.join(", ");
    let out = |value: serde_json::Value, text: String| {
        if cli.json {
            serde_json::to_string_pretty(&value).map(|s| s + "\n")
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
    }
}
