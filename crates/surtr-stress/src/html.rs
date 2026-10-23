//! A run as a self-contained HTML report: no scripts, no external files,
//! light and dark themes. Charts are inline SVG.

use std::fmt::Write as _;

use crate::{
    record::{Record, State},
    runner::Outcome,
};

/// Text for HTML; sensor and CPU names come from the hardware.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `1m 05s` (or `2h 00m`, `4s`) for milliseconds.
pub fn span(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        3600.. => format!("{}h {:02}m", s / 3600, s / 60 % 60),
        60.. => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{s}s"),
    }
}

/// The verdict line and its tone (`ok`, `warn`, `bad`).
pub fn verdict(record: &Record) -> (String, &'static str) {
    match (&record.state, record.report.as_ref().map(|r| &r.outcome)) {
        (State::Running, _) => ("Running".to_owned(), "warn"),
        (State::Interrupted, _) => (
            "Interrupted: the system stopped during the run (reset, freeze or power loss), or \
             Surtr was killed"
                .to_owned(),
            "bad",
        ),
        (State::Done, Some(Outcome::Passed)) => ("Passed: no errors".to_owned(), "ok"),
        (State::Done, Some(Outcome::ErrorsFound)) => (
            "Errors found: the CPU gave wrong answers and is not stable at these settings"
                .to_owned(),
            "bad",
        ),
        (
            State::Done,
            Some(Outcome::TemperatureLimit {
                sensor,
                value,
                limit,
            }),
        ) => (
            format!("Stopped: {sensor} reached {value:.1} °C (limit {limit:.0} °C)"),
            "warn",
        ),
        (State::Done, Some(Outcome::Stopped)) => ("Stopped by the user".to_owned(), "warn"),
        (State::Done, Some(Outcome::WorkerFailed { reason })) => {
            (format!("Failed: {reason}"), "bad")
        }
        (State::Done, None) => ("Done".to_owned(), "warn"),
    }
}

/// A line chart of `points` (seconds, value), with an optional dashed limit.
fn chart(title: &str, unit: &str, points: &[(f64, f64)], limit: Option<f64>) -> String {
    const W: f64 = 640.0;
    const H: f64 = 180.0;
    const PAD: f64 = 36.0;
    if points.len() < 2 {
        return format!(
            "<h2>{}</h2><p class=\"muted\">Not enough data.</p>",
            esc(title)
        );
    }
    let x_max = points.last().map_or(1.0, |p| p.0).max(1.0);
    let mut lo = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut hi = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if let Some(l) = limit {
        hi = hi.max(l);
        lo = lo.min(l);
    }
    if (hi - lo).abs() < 1e-9 {
        hi += 1.0;
        lo -= 1.0;
    }
    let x = |s: f64| PAD + s / x_max * (W - 2.0 * PAD);
    let y = |v: f64| H - PAD / 2.0 - (v - lo) / (hi - lo) * (H - PAD);
    let path: Vec<String> = points
        .iter()
        .map(|(s, v)| format!("{:.1},{:.1}", x(*s), y(*v)))
        .collect();
    let mut svg = format!(
        "<h2>{}</h2><svg viewBox=\"0 0 {W} {H}\" role=\"img\" aria-label=\"{}\">",
        esc(title),
        esc(title)
    );
    for v in [lo, hi] {
        let _ = write!(
            svg,
            "<text x=\"2\" y=\"{:.1}\" class=\"axis\">{v:.0}{unit}</text>",
            y(v) + 4.0
        );
    }
    let _ = write!(
        svg,
        "<text x=\"{:.1}\" y=\"{H}\" class=\"axis\" text-anchor=\"end\">{}</text>",
        W - PAD,
        span((x_max * 1000.0) as u64)
    );
    if let Some(l) = limit {
        let _ = write!(
            svg,
            "<line x1=\"{PAD}\" x2=\"{:.1}\" y1=\"{y:.1}\" y2=\"{y:.1}\" class=\"limit\"/>",
            W - PAD,
            y = y(l)
        );
    }
    let _ = write!(
        svg,
        "<polyline points=\"{}\" class=\"line\"/></svg>",
        path.join(" ")
    );
    svg
}

/// The record as an HTML page.
pub fn report(record: &Record) -> String {
    let (verdict, tone) = verdict(record);
    let mut body = String::new();
    let _ = write!(
        body,
        "<h1>Surtr {} test</h1><p class=\"verdict {tone}\">{}</p>",
        esc(&record.workload.to_uppercase()),
        esc(&verdict)
    );

    let report = record.report.as_ref();
    let length = report.map_or_else(
        || record.timeline.last().map_or(0, |t| t.elapsed_ms),
        |r| r.duration_ms,
    );
    let mut rows = vec![
        ("Run", record.id.clone()),
        ("Started", record.started.clone()),
        (
            "Length",
            format!("{} of {} planned", span(length), span(record.planned_ms)),
        ),
        ("Threads", record.threads.to_string()),
        (
            "CPUs",
            if record.cpus.is_empty() {
                "-".to_owned()
            } else {
                record
                    .cpus
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        ("Machine", record.machine.join(" · ")),
        ("Surtr", record.surtr.clone()),
    ];
    if let Some(agent) = &record.requested_by {
        rows.push(("Requested by", format!("{agent} (confirmed by a person)")));
    }
    let peak = report.and_then(|r| r.peak_temp_c).or_else(|| {
        record
            .timeline
            .iter()
            .filter_map(|t| t.temp_c)
            .reduce(f64::max)
    });
    if let Some(p) = peak {
        rows.push(("Peak temperature", format!("{p:.1} °C")));
    }
    for w in &record.watched {
        rows.push(("Stops at", format!("{} {:.0} °C", w.label, w.stop_at)));
    }
    body.push_str("<table>");
    for (k, v) in rows {
        let _ = write!(body, "<tr><th>{k}</th><td>{}</td></tr>", esc(&v));
    }
    body.push_str("</table>");

    let temps: Vec<(f64, f64)> = record
        .timeline
        .iter()
        .filter_map(|t| Some((t.elapsed_ms as f64 / 1000.0, t.temp_c?)))
        .collect();
    let limit = record.watched.iter().map(|w| w.stop_at).reduce(f64::min);
    body.push_str(&chart("Temperature", " °C", &temps, limit));
    // Throughput per second: a drop under steady load means the clock fell
    // (throttling).
    let rate: Vec<(f64, f64)> = record
        .timeline
        .windows(2)
        .filter_map(|w| {
            let dt = w[1].elapsed_ms.checked_sub(w[0].elapsed_ms)? as f64 / 1000.0;
            (dt > 0.0).then(|| {
                (
                    w[1].elapsed_ms as f64 / 1000.0,
                    w[1].iterations.saturating_sub(w[0].iterations) as f64 / dt,
                )
            })
        })
        .collect();
    body.push_str(&chart(
        "Throughput (iterations per second)",
        "",
        &rate,
        None,
    ));

    if let Some(r) = report.filter(|r| !r.failures.is_empty()) {
        body.push_str(
            "<h2>Wrong answers</h2><table><tr><th>When</th><th>CPU</th><th>Kernel</th>\
             <th>Clock</th></tr>",
        );
        for f in &r.failures {
            let cpu = f
                .cpu
                .map_or_else(|| format!("thread {}", f.thread), |c| c.to_string());
            let clock = f
                .clock_mhz
                .map_or_else(|| "-".to_owned(), |c| format!("{c:.0} MHz"));
            let _ = write!(
                body,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                span(f.elapsed_ms),
                esc(&cpu),
                esc(&f.kernel),
                clock
            );
        }
        body.push_str("</table>");
    }

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Surtr run {id}</title><style>{STYLE}</style></head><body><main>{body}\
<p class=\"muted\">Made by Surtr {ver}. Nothing in this file is sent anywhere.</p></main></body></html>\n",
        id = esc(&record.id),
        ver = esc(&record.surtr),
    )
}

const STYLE: &str = ":root{--bg:#fafaf9;--fg:#1f2328;--muted:#5f6772;--line:#e3e5e8;--accent:#d95a00;\
--ok:#1a7f37;--warn:#9a6700;--bad:#cf222e}\
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--fg:#e6e8eb;--muted:#8b939e;--line:#2a2e35;\
--accent:#ff7a1a;--ok:#3fb950;--warn:#d29922;--bad:#ff5c5c}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);\
font:15px/1.5 system-ui,Segoe UI,Helvetica,Arial,sans-serif}\
main{max-width:760px;margin:0 auto;padding:24px 16px 48px}h1{font-size:24px;margin:0 0 8px}\
h2{font-size:17px;margin:28px 0 8px}.verdict{font-weight:600;margin:0 0 16px}\
.ok{color:var(--ok)}.warn{color:var(--warn)}.bad{color:var(--bad)}.muted{color:var(--muted)}\
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:4px 8px;\
border-bottom:1px solid var(--line);vertical-align:top}th{color:var(--muted);font-weight:500;width:34%}\
svg{width:100%;height:auto}.line{fill:none;stroke:var(--accent);stroke-width:2}\
.limit{stroke:var(--bad);stroke-dasharray:6 4}.axis{fill:var(--muted);font-size:11px}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record::{SCHEMA, Tick},
        runner::{Failure, Report, Watched},
    };

    fn record() -> Record {
        Record {
            schema: SCHEMA,
            id: "20261022-234501-cpu".into(),
            workload: "cpu".into(),
            state: State::Done,
            started: "2026-10-22T23:45:01Z".into(),
            ended: Some("2026-10-22T23:46:01Z".into()),
            planned_ms: 60_000,
            threads: 8,
            cpus: Vec::new(),
            surtr: "0.1.0".into(),
            machine: vec!["CPU <script>".into()],
            watched: vec![Watched {
                id: "hwmon/cpu0/temp1".into(),
                label: "Tctl".into(),
                stop_at: 95.0,
            }],
            requested_by: None,
            errors_seen: 1,
            timeline: (0..5)
                .map(|s| Tick {
                    elapsed_ms: s * 1000,
                    temp_c: Some(60.0 + s as f64),
                    iterations: s * 100,
                })
                .collect(),
            report: Some(Report {
                outcome: Outcome::ErrorsFound,
                threads: 8,
                duration_ms: 5000,
                iterations: vec![50; 8],
                failures: vec![Failure {
                    thread: 3,
                    cpu: Some(3),
                    kernel: "float".into(),
                    elapsed_ms: 2500,
                    clock_mhz: Some(3700.0),
                }],
                peak_temp_c: Some(64.0),
                watched: Vec::new(),
            }),
        }
    }

    #[test]
    fn spans() {
        assert_eq!(span(4_200), "4s");
        assert_eq!(span(65_000), "1m 05s");
        assert_eq!(span(7_200_000), "2h 00m");
    }

    #[test]
    fn report_page() {
        let page = report(&record());
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("Errors found"));
        assert!(page.contains("3700 MHz"));
        assert!(page.contains("Tctl 95 °C"));
        assert_eq!(page.matches("<svg").count(), 2);
        assert!(
            page.contains("CPU &lt;script&gt;"),
            "hardware text is escaped"
        );
        assert!(!page.contains("<script"));
        assert!(!page.contains("http"), "no external resources");
    }

    #[test]
    fn interrupted_run_keeps_its_timeline() {
        let mut r = record();
        r.state = State::Interrupted;
        r.report = None;
        let page = report(&r);
        assert!(page.contains("Interrupted"));
        assert!(page.contains("Peak temperature"));
        assert!(page.contains("<polyline"));
    }
}
