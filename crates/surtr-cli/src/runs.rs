//! `surtr runs`: past stress runs, and HTML reports of them.

use std::{
    fmt::Write as _,
    fs,
    io::{self, BufRead, IsTerminal, Write as _},
    time::Duration,
};

use serde_json::json;
use surtr_core::{Hub, config::Config};
use surtr_model::Timestamp;
use surtr_stress::{
    html::{self, span},
    record::{self, Record, State},
    request::{self, RequestState},
    runner::Report,
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
    let find = |id: &str| -> Result<Record, String> {
        record::pick(records.clone(), id)
            .ok_or_else(|| format!("no run {id}; `surtr runs` lists them"))
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
            let r = &find(id)?;
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
        Some(RunsCommand::Deny { id }) => {
            let mut req = request::load(&dir, id, Timestamp::now().0)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no request {id}"))?;
            if req.state != RequestState::Pending {
                return Err(format!("request {id} is {:?}, not pending", req.state).to_lowercase());
            }
            req.state = RequestState::Denied;
            request::save(&dir, &req).map_err(|e| e.to_string())?;
            Ok(format!("Turned down request {id} from {}.\n", req.client))
        }
        Some(RunsCommand::Compare { a, b }) => {
            let c = surtr_stress::compare::compare(&find(a)?, &find(b)?);
            if json {
                return Ok(serde_json::to_string_pretty(&c).map_err(|e| e.to_string())? + "\n");
            }
            Ok(compare_text(&c))
        }
        Some(RunsCommand::Confirm { .. }) => unreachable!("needs the hardware; see confirm"),
        Some(RunsCommand::Report { id, out }) => {
            let r = &find(id)?;
            let path = out
                .clone()
                .unwrap_or_else(|| format!("surtr-run-{}.html", r.id).into());
            fs::write(&path, html::report(r)).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(format!("Wrote {}\n", path.display()))
        }
    }
}

/// `surtr runs confirm <id>`: a person confirms an AI agent's request, and
/// the run happens here, in this terminal, under the usual limits. Agents
/// can't confirm their own requests: this needs an interactive terminal and
/// a typed "y".
pub fn confirm(hub: &Hub, id: &str, settings: &Config) -> Result<(Report, u8), String> {
    let dir = stress::runs_dir().ok_or("no per-user data directory found")?;
    let mut req = request::load(&dir, id, Timestamp::now().0)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no request {id}"))?;
    if req.state != RequestState::Pending {
        return Err(format!("request {id} is {:?}, not pending", req.state).to_lowercase());
    }
    if !io::stdin().is_terminal() {
        return Err("confirming a stress run needs a person at an interactive terminal".to_owned());
    }
    let ms = |t: &str| crate::history::parse_duration_ms(t).map(Duration::from_millis);
    let opts = stress::Options {
        duration: Duration::from_millis(req.duration_ms),
        max_duration: ms(settings.stress.max_duration.as_deref().unwrap_or("30m"))?,
        max_temp_c: f64::from(settings.stress.max_temp_c.unwrap_or(95)),
        threads: req.threads,
        cpus: None,
        smt: true,
        unmonitored: false,
        requested_by: Some(req.client.clone()),
        animate: stress::animate(false),
    };
    eprintln!(
        "{} asks to run the {} stress test: {} threads for {}.\n\
         It heats the machine and slows everything else down; on an unstable machine it can \
         crash it and lose unsaved work. It stops at the temperature limit (stress.max_temp_c, \
         {} °C), and you can stop it with Ctrl+C.",
        req.client,
        req.workload,
        req.threads
            .map_or_else(|| "all".to_owned(), |t| t.to_string()),
        span(req.duration_ms),
        opts.max_temp_c,
    );
    eprint!("Run it now in this terminal? [y/N] ");
    io::stderr().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        req.state = RequestState::Denied;
        request::save(&dir, &req).map_err(|e| e.to_string())?;
        return Err(format!("turned down request {id}"));
    }
    stress::run(hub, &opts, |run| {
        req.state = RequestState::Started;
        req.run = Some(run.to_owned());
        let _ = request::save(&dir, &req);
    })
}

/// Two runs side by side, with the change from the first to the second.
fn compare_text(c: &surtr_stress::compare::Comparison) -> String {
    use surtr_stress::compare::Summary;
    let mut out = String::new();
    let row = |out: &mut String, name: &str, a: String, b: String, change: String| {
        let _ = writeln!(out, "{name:<20} {a:<26} {b:<26} {change}");
    };
    let cpus = |s: &Summary| {
        let list: Vec<String> = s.cpus.iter().map(u32::to_string).collect();
        if list.is_empty() {
            s.threads.to_string()
        } else {
            format!("{} ({})", s.threads, list.join(","))
        }
    };
    let rate = |s: &Summary| {
        s.rate
            .map_or_else(|| "-".to_owned(), |r| format!("{r:.0}/s"))
    };
    let temp = |t: Option<f64>| t.map_or_else(|| "-".to_owned(), |t| format!("{t:.1} °C"));
    let pct = |p: Option<f64>| p.map_or_else(String::new, |p| format!("{p:+.1}%"));
    let deg = |d: Option<f64>| d.map_or_else(String::new, |d| format!("{d:+.1} °C"));
    row(&mut out, "", c.a.id.clone(), c.b.id.clone(), String::new());
    row(
        &mut out,
        "Result",
        c.a.result.clone(),
        c.b.result.clone(),
        String::new(),
    );
    row(
        &mut out,
        "Length",
        span(c.a.length_ms),
        span(c.b.length_ms),
        String::new(),
    );
    row(
        &mut out,
        "Threads (CPUs)",
        cpus(&c.a),
        cpus(&c.b),
        String::new(),
    );
    row(
        &mut out,
        "Throughput",
        rate(&c.a),
        rate(&c.b),
        pct(c.rate_change_pct),
    );
    row(
        &mut out,
        "Peak temperature",
        temp(c.a.peak_temp_c),
        temp(c.b.peak_temp_c),
        deg(c.peak_temp_change_c),
    );
    row(
        &mut out,
        "Average temperature",
        temp(c.a.avg_temp_c),
        temp(c.b.avg_temp_c),
        deg(c.avg_temp_change_c),
    );
    let sag = |s: &Summary| {
        s.sag_pct
            .map_or_else(|| "-".to_owned(), |p| format!("{p:.1}%"))
    };
    row(
        &mut out,
        "Sag (throttling)",
        sag(&c.a),
        sag(&c.b),
        String::new(),
    );
    row(
        &mut out,
        "Errors",
        c.a.errors.to_string(),
        c.b.errors.to_string(),
        String::new(),
    );
    for w in &c.warnings {
        let _ = writeln!(out, "! {w}");
    }
    out
}
