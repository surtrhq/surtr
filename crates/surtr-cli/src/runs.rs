//! `surtr runs`: past stress runs, and HTML reports of them.

use std::{fmt::Write as _, fs};

use serde_json::json;
use surtr_stress::{
    html::{self, span},
    record::{self, Record, State},
};

use crate::{RunsCommand, stress};

fn length(r: &Record) -> u64 {
    r.report.as_ref().map_or_else(
        || r.timeline.last().map_or(0, |t| t.elapsed_ms),
        |rep| rep.duration_ms,
    )
}

fn peak(r: &Record) -> Option<f64> {
    r.report
        .as_ref()
        .and_then(|rep| rep.peak_temp_c)
        .or_else(|| r.timeline.iter().filter_map(|t| t.temp_c).reduce(f64::max))
}

/// A short result word for the list.
fn result(r: &Record) -> &'static str {
    match (r.state, r.report.as_ref().map(|rep| &rep.outcome)) {
        (State::Running, _) => "running",
        (State::Interrupted, _) => "interrupted",
        (State::Done, Some(surtr_stress::runner::Outcome::Passed)) => "passed",
        (State::Done, Some(surtr_stress::runner::Outcome::ErrorsFound)) => "errors",
        (State::Done, Some(surtr_stress::runner::Outcome::TemperatureLimit { .. })) => "temp limit",
        (State::Done, Some(surtr_stress::runner::Outcome::Stopped)) => "stopped",
        (State::Done, Some(surtr_stress::runner::Outcome::WorkerFailed { .. })) => "failed",
        (State::Done, None) => "done",
    }
}

/// Runs the `surtr runs` subcommand.
pub fn command(command: Option<&RunsCommand>, json: bool) -> Result<String, String> {
    let dir = stress::runs_dir().ok_or("no per-user data directory found")?;
    let (records, interrupted) = record::load_all(&dir).map_err(|e| e.to_string())?;
    for id in &interrupted {
        eprintln!(
            "surtr: run {id} was interrupted: the system stopped during it (reset, freeze or \
             power loss), or Surtr was killed."
        );
    }
    let find = |id: &str| -> Result<&Record, String> {
        let found = if id == "latest" {
            records.last()
        } else {
            records.iter().find(|r| r.id == id)
        };
        found.ok_or_else(|| format!("no run {id}; `surtr runs` lists them"))
    };
    match command {
        None => {
            if json {
                return Ok(serde_json::to_string_pretty(&json!({ "runs": records }))
                    .map_err(|e| e.to_string())?
                    + "\n");
            }
            if records.is_empty() {
                return Ok("No stress runs yet; `surtr stress cpu` starts one.\n".to_owned());
            }
            let mut out = format!(
                "{:<24} {:<20} {:>8}  {:<11} {:>7}\n",
                "Run", "Started (UTC)", "Length", "Result", "Peak"
            );
            for r in &records {
                let started = r.started.replace('T', " ").trim_end_matches('Z').to_owned();
                let _ = writeln!(
                    out,
                    "{:<24} {:<20} {:>8}  {:<11} {:>7}",
                    r.id,
                    started,
                    span(length(r)),
                    result(r),
                    peak(r).map_or_else(|| "-".to_owned(), |p| format!("{p:.0} °C")),
                );
            }
            Ok(out)
        }
        Some(RunsCommand::Show { id }) => {
            let r = find(id)?;
            if json {
                return Ok(serde_json::to_string_pretty(r).map_err(|e| e.to_string())? + "\n");
            }
            let mut out = format!("{}\n{}\n", r.id, html::verdict(r).0);
            match &r.report {
                Some(rep) => out.push_str(&stress::describe(rep)),
                None => {
                    let _ = writeln!(
                        out,
                        "  {} threads · {} recorded{}",
                        r.threads,
                        span(length(r)),
                        peak(r).map_or_else(String::new, |p| format!(" · peak {p:.1} °C"))
                    );
                }
            }
            Ok(out)
        }
        Some(RunsCommand::Report { id, out }) => {
            let r = find(id)?;
            let path = out
                .clone()
                .unwrap_or_else(|| format!("surtr-run-{}.html", r.id).into());
            fs::write(&path, html::report(r)).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(format!("Wrote {}\n", path.display()))
        }
    }
}
