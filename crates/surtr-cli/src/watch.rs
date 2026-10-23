//! `surtr watch`: a live table of sensors with trends in the normal terminal,
//! redrawn in place. With `--json` it streams one JSON object per line
//! instead; piped text output prints each frame after the previous one.

use std::{
    collections::HashMap,
    error::Error,
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    thread,
    time::Duration,
};

use crossterm::{
    cursor::MoveUp,
    queue,
    terminal::{Clear, ClearType},
};
use serde_json::json;
use surtr_core::{Hub, Sample};
use surtr_view::{Role, format, health};

use crate::style::paint;

const TREND: usize = 30;

/// Whether `s` matches any filter (case-insensitive substring of its id,
/// label or device). Without filters, everything but per-thread sensors.
fn selected(s: &Sample, filters: &[String]) -> bool {
    let d = &s.descriptor;
    if filters.is_empty() {
        return !d.id.0.contains("thread");
    }
    let hay = [&d.id.0, &d.label, &d.device].map(|t| t.to_lowercase());
    filters
        .iter()
        .any(|f| hay.iter().any(|h| h.contains(&f.to_lowercase())))
}

fn frame(
    samples: &[&Sample],
    trends: &HashMap<(String, String), Vec<f64>>,
    interval: Duration,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        paint(
            Role::Muted,
            &format!(
                "surtr watch · every {} ms · Ctrl+C to stop",
                interval.as_millis()
            )
        )
    );
    let mut device = "";
    for s in samples {
        let d = &s.descriptor;
        if d.device != device {
            device = &d.device;
            let _ = writeln!(out, "{}", paint(Role::Heading, device));
        }
        let (text, role) = match s.value {
            Some(v) => (
                format::value(d.unit, v),
                health::level(d, v).map_or(Role::Text, |l| l.role()),
            ),
            None => (format::NO_READING.to_owned(), Role::Missing),
        };
        let trend = trends
            .get(&(d.source.provider.clone(), d.id.0.clone()))
            .map(|t| format::spark(t.iter().copied(), TREND, format::spark_range(d.unit)))
            .unwrap_or_default();
        let label: String = d.label.chars().take(28).collect();
        let _ = writeln!(
            out,
            "  {label:<28} {} {}",
            paint(role, &format!("{text:>13}")),
            paint(Role::Accent, &trend)
        );
    }
    out
}

/// Runs until interrupted, or for `count` updates.
pub fn run(
    hub: &Hub,
    filters: &[String],
    interval: Duration,
    count: Option<u64>,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    let live = !json && std::io::stdout().is_terminal();
    let mut trends: HashMap<(String, String), Vec<f64>> = HashMap::new();
    let mut previous_lines = 0u16;
    // Rate sensors need a first sample to compare with.
    hub.read();
    let mut done = 0;
    while count.is_none_or(|c| done < c) {
        thread::sleep(interval);
        let samples = hub.read();
        let chosen: Vec<&Sample> = samples.iter().filter(|s| selected(s, filters)).collect();
        if chosen.is_empty() {
            return Err("no sensor matches; run `surtr sensors` to see the ids and labels".into());
        }
        for s in &chosen {
            if let Some(v) = s.value {
                let key = (
                    s.descriptor.source.provider.clone(),
                    s.descriptor.id.0.clone(),
                );
                let trend = trends.entry(key).or_default();
                trend.push(v);
                if trend.len() > TREND {
                    trend.remove(0);
                }
            }
        }
        let mut stdout = anstream::stdout();
        if json {
            let values: Vec<_> = chosen
                .iter()
                .map(|s| {
                    json!({
                        "provider": s.descriptor.source.provider,
                        "id": s.descriptor.id,
                        "value": s.value,
                        "at": s.at,
                    })
                })
                .collect();
            writeln!(stdout, "{}", json!({ "values": values }))?;
        } else {
            let text = frame(&chosen, &trends, interval);
            if live && previous_lines > 0 {
                queue!(
                    std::io::stdout(),
                    MoveUp(previous_lines),
                    Clear(ClearType::FromCursorDown)
                )?;
            }
            write!(stdout, "{text}")?;
            if !live {
                writeln!(stdout)?;
            }
            previous_lines = u16::try_from(text.lines().count()).unwrap_or(u16::MAX);
        }
        stdout.flush()?;
        done += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_model::{SensorDescriptor, SensorId, SensorKind, Source};

    fn sample(id: &str, label: &str) -> Sample {
        Sample {
            descriptor: SensorDescriptor {
                id: SensorId::new(id),
                kind: SensorKind::Load,
                unit: SensorKind::Load.unit(),
                device: "cpu".to_owned(),
                label: label.to_owned(),
                source: Source::new("t", "t"),
                limits: Default::default(),
            },
            value: Some(1.0),
            at: None,
        }
    }

    #[test]
    fn default_selection_skips_threads() {
        assert!(selected(&sample("cpu/load", "Total load"), &[]));
        assert!(!selected(&sample("cpu/thread3/load", "Thread 3 load"), &[]));
    }

    #[test]
    fn filters_match_id_label_or_device() {
        let s = sample("cpu/thread3/load", "Thread 3 load");
        assert!(selected(&s, &["THREAD 3".to_owned()]));
        assert!(selected(&s, &["cpu".to_owned()]));
        assert!(!selected(&s, &["gpu".to_owned()]));
    }
}
