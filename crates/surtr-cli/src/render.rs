//! Text layout of the shared view models (`surtr-view`), styled by role.
//! Styling is stripped automatically when output is piped or `NO_COLOR` is
//! set (see `style`).

use std::fmt::Write;

use surtr_model::{Capabilities, GpuProcess, SystemInfo, ThrottleState};
use surtr_view::{
    Cell, Level, Role, Section, Status, capability_rows, process_rows, sensor_sections,
    system_sections, throttle_rows,
};

use crate::style::paint;

fn heading(out: &mut String, text: &str) {
    // Writing to a String can't fail.
    let _ = writeln!(out, "{}", paint(Role::Heading, text));
}

fn footer(out: &mut String, text: &str) {
    let _ = writeln!(out, "\n{}", paint(Role::Muted, text));
}

/// A cell left-aligned in `width` columns.
fn left(cell: &Cell, width: usize) -> String {
    paint(cell.role, &format!("{:<width$}", cell.text))
}

/// A cell right-aligned in `width` columns.
fn right(cell: &Cell, width: usize) -> String {
    paint(cell.role, &format!("{:>width$}", cell.text))
}

fn sections(out: &mut String, sections: &[Section], row: impl Fn(&mut String, &surtr_view::Row)) {
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        heading(out, &section.title);
        for r in &section.rows {
            row(out, r);
        }
    }
}

/// The `surtr info` report.
pub fn info(provider: &str, system: &SystemInfo) -> String {
    let mut out = String::new();
    sections(&mut out, &system_sections(system), |out, row| {
        let label = paint(Role::Muted, &format!("{:<14}", row.label));
        let _ = writeln!(
            out,
            "  {label}{}",
            paint(row.cells[0].role, &row.cells[0].text)
        );
    });
    footer(
        &mut out,
        &format!("Providers: {provider}. Run with --json to see where each value came from."),
    );
    out
}

/// The `surtr caps` report: every feature, including ones no provider
/// mentions.
pub fn caps(provider: &str, capabilities: &Capabilities) -> String {
    let mut out = String::new();
    for row in capability_rows(capabilities) {
        let _ = writeln!(
            out,
            "{:<22}{}",
            row.label,
            paint(row.cells[0].role, &row.cells[0].text)
        );
        if let Some(fix) = row.cells.get(1) {
            let _ = writeln!(
                out,
                "{:<22}{}",
                "",
                paint(fix.role, &format!("→ {}", fix.text))
            );
        }
    }
    footer(&mut out, &format!("Providers: {provider}."));
    out
}

/// The `surtr sensors` report, grouped by device in provider order.
pub fn sensors(provider: &str, samples: &[surtr_core::Sample]) -> String {
    let mut out = String::new();
    if samples.is_empty() {
        out.push_str("No sensors available on this machine yet.\n");
    }
    sections(&mut out, &sensor_sections(samples), |out, row| {
        let _ = writeln!(out, "  {:<32}{}", row.label, right(&row.cells[0], 14));
    });
    footer(
        &mut out,
        &format!("Providers: {provider}. Run with --json to see where each value came from."),
    );
    out
}

/// The `surtr gpu procs` report.
pub fn gpu_processes(processes: &[GpuProcess]) -> String {
    if processes.is_empty() {
        return "No processes are using a GPU.\n".to_owned();
    }
    let mut out = String::new();
    for (i, section) in process_rows(processes).iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        heading(&mut out, &section.title);
        let header = format!("  {:<10}{:<11}{:>12}  Name", "PID", "Type", "Memory");
        let _ = writeln!(out, "{}", paint(Role::Muted, &header));
        for row in &section.rows {
            let [kind, memory, name] = &row.cells[..] else {
                continue;
            };
            let _ = writeln!(
                out,
                "  {:<10}{}{}  {}",
                row.label,
                left(kind, 11),
                right(memory, 12),
                paint(name.role, &name.text)
            );
        }
    }
    out
}

/// The `surtr gpu throttle` report.
pub fn throttle(states: &[ThrottleState]) -> String {
    if states.is_empty() {
        return "No GPU reports throttle reasons.\n".to_owned();
    }
    let mut out = String::new();
    for row in throttle_rows(states) {
        let reasons: Vec<String> = row.cells.iter().map(|c| paint(c.role, &c.text)).collect();
        let _ = writeln!(out, "{:<12}{}", row.label, reasons.join(", "));
    }
    out
}

/// The `surtr status` report.
pub fn status(status: &Status) -> String {
    let mut out = String::new();
    let (word, role) = match status.level {
        Level::Ok => ("OK", Role::Ok),
        Level::Warn => ("WARNING", Role::Warn),
        Level::Critical => ("CRITICAL", Role::Critical),
    };
    let _ = writeln!(
        out,
        "{} {}\n",
        paint(Role::Heading, "Status:"),
        paint(role, word)
    );
    for row in &status.parts {
        let label = paint(Role::Muted, &format!("{:<10}", row.label));
        let _ = writeln!(
            out,
            "  {label} {}",
            paint(row.cells[0].role, &row.cells[0].text)
        );
    }
    if !status.findings.is_empty() {
        out.push('\n');
        heading(&mut out, "Findings");
        for f in &status.findings {
            let (mark, role) = match f.level {
                Some(Level::Critical) => ("!!", Role::Critical),
                Some(Level::Warn) => ("! ", Role::Warn),
                Some(Level::Ok) => ("  ", Role::Ok),
                None => ("i ", Role::Muted),
            };
            let _ = writeln!(out, "  {} {}", paint(role, mark), f.message);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{Provider, mock::MockProvider};
    use surtr_core::Sample;
    use surtr_model::Feature;

    // The tests check text layout; strip styling first. These shadow the
    // functions imported from `super`.
    fn plain(s: String) -> String {
        anstream::adapter::strip_str(&s).to_string()
    }
    fn info(p: &str, s: &SystemInfo) -> String {
        plain(super::info(p, s))
    }
    fn caps(p: &str, c: &Capabilities) -> String {
        plain(super::caps(p, c))
    }
    fn sensors(p: &str, s: &[Sample]) -> String {
        plain(super::sensors(p, s))
    }
    fn gpu_processes(p: &[GpuProcess]) -> String {
        plain(super::gpu_processes(p))
    }
    fn throttle(t: &[ThrottleState]) -> String {
        plain(super::throttle(t))
    }

    #[test]
    fn sensors_grouped_by_device() {
        let mock = MockProvider::desktop();
        let descriptors = mock.sensors().unwrap();
        let readings = mock.read_sensors().unwrap();
        let values: Vec<Sample> = descriptors
            .iter()
            .map(|descriptor| Sample {
                descriptor: descriptor.clone(),
                value: readings
                    .iter()
                    .find(|r| r.id == descriptor.id)
                    .map(|r| r.value)
                    .filter(|_| descriptor.id.0 != "gpu0/fan"),
                at: None,
            })
            .collect();
        let out = sensors("mock", &values);
        assert!(out.starts_with("cpu0\n  Package temperature"), "{out}");
        assert!(out.contains("\n\ngpu0\n"), "{out}");
        // First mock step: every sine starts at its base value.
        let line = |label: &str, value: &str| format!("  {label:<32}{value:>14}\n");
        for expected in [
            line("Package temperature", "55.0 °C"),
            line("VRAM used", "6.0 GiB"),
            line("Fan speed", "not available"),
        ] {
            assert!(out.contains(&expected), "missing {expected:?} in\n{out}");
        }
    }

    #[test]
    fn no_sensors_message() {
        assert!(sensors("x", &[]).starts_with("No sensors available"));
    }

    #[test]
    fn gpu_process_table() {
        let mock = MockProvider::desktop();
        let mut procs = mock.gpu_processes().unwrap();
        let out = gpu_processes(&procs);
        assert!(out.starts_with("gpu0\n  PID"), "{out}");
        let row = format!(
            "  {:<10}{:<11}{:>12}  {}\n",
            4242, "compute", "4.0 GiB", "mock-inference"
        );
        assert!(out.contains(&row), "{out}");
        procs[0].used_memory_bytes = None;
        procs[0].name = None;
        assert!(gpu_processes(&procs).contains("n/a  ?"));
        assert_eq!(gpu_processes(&[]), "No processes are using a GPU.\n");
    }

    #[test]
    fn throttle_lines() {
        let mut states = MockProvider::desktop().throttle_reasons().unwrap();
        assert_eq!(throttle(&states), "gpu0        power limit\n");
        states[0].reasons.clear();
        assert_eq!(
            throttle(&states),
            "gpu0        none (running unrestricted)\n"
        );
    }

    #[test]
    fn info_shows_values_and_missing_fields() {
        let mock = MockProvider::desktop();
        let mut system = mock.system_info().unwrap();
        system.cpus[0].max_clock_mhz = None;
        let out = info("mock", &system);
        assert!(out.contains("  Name          Mock 8-Core Processor\n"));
        assert!(out.contains("  Threads       16\n"));
        assert!(out.contains("  Base clock    3800 MHz\n"));
        assert!(out.contains("  Max clock     not available\n"));
        assert!(out.contains("  Total         32.0 GiB\n"));
        assert!(out.contains("GPU 0"));
        assert!(
            out.ends_with("Providers: mock. Run with --json to see where each value came from.\n")
        );
    }

    #[test]
    fn caps_show_remedies_under_the_status() {
        let mut capabilities = Capabilities::default();
        capabilities.set_with(
            Feature::CpuTemperature,
            surtr_model::Support::Unsupported {
                reason: "no driver".into(),
            },
            Some(
                surtr_model::Remedy::new("Load the coretemp kernel module")
                    .command("sudo modprobe coretemp"),
            ),
        );
        let out = caps("test", &capabilities);
        assert!(
            out.contains(&format!(
                "CPU temperature       unsupported (no driver)\n{:<22}→ Load the coretemp kernel module: sudo modprobe coretemp\n",
                ""
            )),
            "{out}"
        );
    }

    #[test]
    fn caps_lists_every_feature() {
        let out = caps("mock", &MockProvider::desktop().capabilities());
        assert_eq!(out.lines().count(), Feature::ALL.len() + 2);
        assert!(out.contains("CPU identity          available\n"));
        assert!(out.contains("Memory timings        needs privileged backend\n"));
        assert!(
            out.contains("Fan control           unsupported (the mock provider is read-only)\n")
        );
        assert!(out.contains("GPU tuning            unknown\n"));
    }
}
