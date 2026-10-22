//! `surtr setup`: runs the fixes that `surtr caps` suggests, on request
//! (issue #24, level 3). Every command is shown first and confirmed one by
//! one; nothing runs without a terminal to ask on.
//!
//! Only commands of a few known shapes run: loading a kernel module, running
//! lm-sensors' detection, and installing a named package with the system
//! package manager. They run as an argument list, never through a shell.
//! Anything else is shown for the user to do by hand. The daemon and MCP
//! never reach this code.

use std::{
    fmt::Write as _,
    io::{self, BufRead, IsTerminal, Write as _},
    process::Command,
};

use surtr_model::{Capabilities, Capability, Support};
use surtr_view::format;

/// A fix Surtr may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What it fixes, for example "CPU temperature".
    pub feature: String,
    /// Why it doesn't work now.
    pub reason: String,
    /// What the command does.
    pub text: String,
    /// The command, as arguments.
    pub argv: Vec<String>,
}

/// A fix the user has to do by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manual {
    /// What it fixes.
    pub feature: String,
    /// Why it doesn't work now.
    pub reason: String,
    /// What to do, with the command or link if there is one.
    pub text: String,
}

fn name_ok(s: &str, extra: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// `command` as arguments, if it has one of the shapes `surtr setup` runs.
pub fn allowed(command: &str) -> Option<Vec<String>> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let ok = match words.as_slice() {
        ["sudo", "modprobe", module] => name_ok(module, "_-"),
        ["sudo", "sensors-detect"] => true,
        ["winget", "install", "--exact", "--id", id] => name_ok(id, ".-_"),
        [
            "sudo",
            "apt" | "apt-get" | "dnf" | "zypper",
            "install",
            packages @ ..,
        ]
        | ["sudo", "pacman", "-S", "--needed", packages @ ..] => {
            !packages.is_empty() && packages.iter().all(|p| name_ok(p, ".+-_"))
        }
        _ => false,
    };
    ok.then(|| words.iter().map(|w| (*w).to_owned()).collect())
}

/// The fixes for everything that doesn't work, split into those Surtr may
/// run and those to do by hand. The same command is listed once.
pub fn plan(caps: &Capabilities) -> (Vec<Step>, Vec<Manual>) {
    let mut steps: Vec<Step> = Vec::new();
    let mut manual = Vec::new();
    for Capability {
        feature,
        support,
        remedy,
    } in &caps.0
    {
        let Some(remedy) = remedy else { continue };
        let reason = match support {
            Support::Available | Support::NoHardware => continue,
            Support::Unsupported { reason } => reason.clone(),
            other => format::support(Some(other)).0,
        };
        let feature = format::feature(*feature).to_owned();
        match remedy.command.as_deref().and_then(allowed) {
            Some(argv) if !steps.iter().any(|s| s.argv == argv) => steps.push(Step {
                feature,
                reason,
                text: remedy.text.clone(),
                argv,
            }),
            Some(_) => {}
            None => manual.push(Manual {
                feature,
                reason,
                text: format::remedy(remedy),
            }),
        }
    }
    (steps, manual)
}

/// The plan as text.
pub fn describe(steps: &[Step], manual: &[Manual]) -> String {
    let mut out = String::new();
    if steps.is_empty() && manual.is_empty() {
        out.push_str("Nothing to set up: Surtr knows no fix for anything missing here.\n");
        return out;
    }
    for (i, s) in steps.iter().enumerate() {
        let _ = writeln!(out, "{}. {}: {}", i + 1, s.feature, s.reason);
        let _ = writeln!(out, "   {}", s.text);
        let _ = writeln!(out, "   $ {}", s.argv.join(" "));
    }
    if !manual.is_empty() {
        if !steps.is_empty() {
            out.push('\n');
        }
        out.push_str("To do by hand:\n");
        for m in manual {
            let _ = writeln!(out, "- {}: {}", m.feature, m.reason);
            let _ = writeln!(out, "  {}", m.text);
        }
    }
    out
}

/// Shows the plan and, unless `dry_run`, runs each step the user confirms.
pub fn run(caps: &Capabilities, dry_run: bool) -> io::Result<String> {
    let (steps, manual) = plan(caps);
    let mut out = describe(&steps, &manual);
    if steps.is_empty() || dry_run {
        return Ok(out);
    }
    if !io::stdin().is_terminal() {
        out.push_str("\nNot run: surtr setup asks before each command and needs a terminal.\n");
        return Ok(out);
    }
    print!("{out}");
    let mut stdin = io::stdin().lock();
    let mut ran = 0;
    for (i, step) in steps.iter().enumerate() {
        print!("\nRun step {} ($ {})? [y/N] ", i + 1, step.argv.join(" "));
        io::stdout().flush()?;
        let mut answer = String::new();
        stdin.read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Skipped.");
            continue;
        }
        // The package manager or sudo asks for elevation itself.
        match Command::new(&step.argv[0]).args(&step.argv[1..]).status() {
            Ok(status) if status.success() => {
                ran += 1;
                println!("Done.");
            }
            Ok(status) => println!("Failed ({status})."),
            Err(e) => println!("Could not start {}: {e}.", step.argv[0]),
        }
    }
    let mut end = format!("\n{ran} of {} steps ran.", steps.len());
    if ran > 0 {
        end.push_str(" Run `surtr caps` to check the result.");
        if steps
            .iter()
            .any(|s| s.argv.get(1).is_some_and(|w| w == "modprobe"))
        {
            end.push_str(
                " A loaded kernel module lasts until the next restart; to keep it, add its \
                 name to a file in /etc/modules-load.d/.",
            );
        }
    }
    Ok(end + "\n")
}

#[cfg(test)]
mod tests {
    use surtr_model::{Feature, Remedy};

    use super::*;

    #[test]
    fn allowlist() {
        assert!(allowed("sudo modprobe coretemp").is_some());
        assert!(allowed("sudo sensors-detect").is_some());
        assert!(allowed("sudo apt install lm-sensors hwdata").is_some());
        assert!(allowed("sudo pacman -S --needed lm_sensors").is_some());
        assert!(allowed("winget install --exact --id Vendor.Tool").is_some());
        for bad in [
            "",
            "rm -rf /",
            "sudo modprobe",
            "sudo modprobe -r nvidia",
            "sudo modprobe coretemp; rm -rf /",
            "sudo modprobe $(id)",
            "sudo apt install",
            "sudo apt install --reinstall nvidia-driver",
            "sudo apt remove lm-sensors",
            "curl https://example.invalid | sh",
            "winget install --id Vendor.Tool",
        ] {
            assert_eq!(allowed(bad), None, "{bad}");
        }
    }

    fn unsupported(feature: Feature, command: Option<&str>, url: Option<&str>) -> Capability {
        let mut remedy = Remedy::new("Fix it");
        remedy.command = command.map(str::to_owned);
        remedy.url = url.map(str::to_owned);
        Capability {
            feature,
            support: Support::Unsupported {
                reason: "missing".to_owned(),
            },
            remedy: Some(remedy),
        }
    }

    #[test]
    fn plans_runnable_and_manual_fixes() {
        let caps = Capabilities(vec![
            unsupported(
                Feature::CpuTemperature,
                Some("sudo modprobe coretemp"),
                None,
            ),
            // The same command for another feature is listed once.
            unsupported(Feature::CpuPower, Some("sudo modprobe coretemp"), None),
            unsupported(Feature::BoardSensors, Some("sudo sensors-detect"), None),
            unsupported(
                Feature::GpuSensors,
                None,
                Some("https://www.nvidia.com/drivers"),
            ),
            unsupported(Feature::CpuVoltage, Some("echo not allowed"), None),
            Capability {
                feature: Feature::CpuLoad,
                support: Support::Available,
                remedy: Some(Remedy::new("ignored").command("sudo modprobe x")),
            },
        ]);
        let (steps, manual) = plan(&caps);
        let argv: Vec<String> = steps.iter().map(|s| s.argv.join(" ")).collect();
        assert_eq!(argv, ["sudo modprobe coretemp", "sudo sensors-detect"]);
        assert_eq!(manual.len(), 2);
        let text = describe(&steps, &manual);
        assert!(text.contains("1. CPU temperature: missing"), "{text}");
        assert!(text.contains("   $ sudo sensors-detect"), "{text}");
        assert!(text.contains("To do by hand:"), "{text}");
        assert!(text.contains("https://www.nvidia.com/drivers"), "{text}");
    }

    #[test]
    fn nothing_to_do() {
        let (steps, manual) = plan(&Capabilities::default());
        assert!(describe(&steps, &manual).starts_with("Nothing to set up"));
    }
}
