//! Drawing. Pure: everything comes from [`App`], so tests render it into an
//! in-memory terminal and snapshot the result.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Cell, Clear, LineGauge, Padding, Paragraph, Row, Table, Wrap},
};
use surtr_core::Sample;
use surtr_model::{SensorKind, Unit};
use surtr_view::{Level, Role, capability_rows, format, sensor_sections, system_sections};

use crate::{
    app::{App, HISTORY, Tab},
    colors::Palette,
};

const BRAND: &str = " SURTR ";
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The tab whose title covers terminal column `column` of the top row.
pub fn tab_at(column: u16) -> Option<Tab> {
    let mut start = BRAND.len() as u16 + 1;
    for tab in Tab::ALL {
        let width = tab.title().len() as u16 + 4;
        if (start..start + width).contains(&column) {
            return Some(tab);
        }
        start += width;
    }
    None
}

/// The last `width` values as block characters, scaled to `range` or to the
/// values' own span.
pub fn spark(
    values: impl IntoIterator<Item = f64>,
    width: usize,
    range: Option<(f64, f64)>,
) -> String {
    let values: Vec<f64> = values.into_iter().collect();
    let values = &values[values.len().saturating_sub(width)..];
    if values.is_empty() {
        return String::new();
    }
    let (lo, hi) = range.unwrap_or_else(|| {
        let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lo, hi)
    });
    values
        .iter()
        .map(|v| {
            if hi <= lo {
                return BARS[0];
            }
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            BARS[((t * 7.0).round() as usize).min(7)]
        })
        .collect()
}

fn range_for(unit: Unit) -> Option<(f64, f64)> {
    (unit == Unit::Percent).then_some((0.0, 100.0))
}

fn block<'a>(title: &'a str, p: &Palette) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(p.style(Role::Muted))
        .title(Span::styled(format!(" {title} "), p.style(Role::Heading)))
}

/// Draws the whole screen.
pub fn draw(f: &mut Frame, app: &App, p: &Palette) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, header, app, p);
    match app.tab {
        Tab::Overview => draw_overview(f, body, app, p),
        Tab::Sensors => draw_sensors(f, body, app, p),
        Tab::Gpu => draw_gpus(f, body, app, p),
        Tab::Capabilities => draw_capabilities(f, body, app, p),
    }
    draw_footer(f, footer, app, p);
    if app.help {
        draw_help(f, f.area(), p);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let mut spans = vec![
        Span::styled(
            BRAND,
            p.style(Role::Accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
        Span::raw(" "),
    ];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let text = format!(" {} {} ", i + 1, tab.title());
        let style = if *tab == app.tab {
            p.style(Role::Accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            p.style(Role::Muted)
        };
        spans.push(Span::styled(text, style));
    }
    let (word, role) = match app.status.level {
        Level::Ok => ("OK", Role::Ok),
        Level::Warn => ("WARNING", Role::Warn),
        Level::Critical => ("CRITICAL", Role::Critical),
    };
    let badge = format!(" {word} ");
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(badge.len() as u16)])
            .areas(area);
    f.render_widget(Line::from(spans), left);
    f.render_widget(
        Span::styled(
            badge,
            p.style(role)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        ),
        right,
    );
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let keys = " q quit  ←/→ tabs  1-4 views  ↑/↓ scroll  ? help";
    let providers = format!("{} ", app.providers.join(", "));
    let [left, right] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(providers.chars().count() as u16),
    ])
    .areas(area);
    f.render_widget(Span::styled(keys, p.style(Role::Muted)), left);
    f.render_widget(Span::styled(providers, p.style(Role::Muted)), right);
}

/// The whole-CPU load sample, or a GPU's load sample.
fn load_of<'a>(app: &'a App, device: Option<&str>) -> Option<&'a Sample> {
    app.samples.iter().find(|s| {
        let d = &s.descriptor;
        d.kind == SensorKind::Load
            && match device {
                None => d.device.starts_with("cpu") && !d.id.0.contains("thread"),
                Some(dev) => d.device == dev && d.label == "GPU load",
            }
    })
}

fn trend_line<'a>(app: &App, sample: &Sample, width: usize, p: &Palette) -> Line<'a> {
    let bars = app
        .trend(sample)
        .map(|t| spark(t.iter().copied(), width, Some((0.0, 100.0))))
        .unwrap_or_default();
    Line::from(vec![
        Span::raw(" ".repeat(11)),
        Span::styled(bars, p.style(Role::Accent)),
    ])
}

fn draw_overview(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let max = (area.height / 3 + 1).max(3);
    let findings_height = (app.status.findings.len() as u16 + 2).clamp(3, max);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(findings_height)]).areas(area);
    let (health, system) = if top.width >= 100 {
        let [a, b] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(top);
        (a, b)
    } else {
        let [a, b] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(top);
        (a, b)
    };

    // Health: one line per part, with a load trend under the CPU and GPUs.
    let width = health.width.saturating_sub(14) as usize;
    let mut lines = Vec::new();
    for row in &app.status.parts {
        lines.push(Line::from(vec![
            Span::styled(format!(" {:<10}", row.label), p.style(Role::Muted)),
            Span::styled(row.cells[0].text.clone(), p.style(row.cells[0].role)),
        ]));
        match row.label.as_str() {
            "CPU" => lines.extend(load_of(app, None).map(|s| trend_line(app, s, width, p))),
            "Memory" => {}
            gpu => lines.extend(load_of(app, Some(gpu)).map(|s| trend_line(app, s, width, p))),
        }
    }
    f.render_widget(Paragraph::new(lines).block(block("Health", p)), health);

    // System: the info sections, flattened.
    let mut lines = Vec::new();
    for section in system_sections(&app.system) {
        lines.push(Line::styled(
            format!(" {}", section.title),
            p.style(Role::Heading),
        ));
        for row in section.rows {
            lines.push(Line::from(vec![
                Span::styled(format!("   {:<13}", row.label), p.style(Role::Muted)),
                Span::styled(row.cells[0].text.clone(), p.style(row.cells[0].role)),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(block("System", p))
            .scroll((app.scroll as u16, 0)),
        system,
    );

    // Findings.
    let lines: Vec<Line> = if app.status.findings.is_empty() {
        vec![Line::styled(" No problems found.", p.style(Role::Ok))]
    } else {
        app.status
            .findings
            .iter()
            .map(|finding| {
                let (mark, role) = match finding.level {
                    Some(Level::Critical) => ("!!", Role::Critical),
                    Some(Level::Warn) => ("! ", Role::Warn),
                    Some(Level::Ok) => ("  ", Role::Ok),
                    None => ("i ", Role::Muted),
                };
                Line::from(vec![
                    Span::styled(format!(" {mark} "), p.style(role)),
                    Span::raw(finding.message.clone()),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(block("Findings", p))
            .wrap(Wrap { trim: false }),
        bottom,
    );
}

fn draw_sensors(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    if app.samples.is_empty() {
        let text = Paragraph::new(" No sensors available on this machine yet.")
            .style(p.style(Role::Missing))
            .block(block("Sensors", p));
        f.render_widget(text, area);
        return;
    }
    let trend_width = 24usize.min(HISTORY);
    let mut rows = Vec::new();
    let sections = sensor_sections(&app.samples);
    let mut samples = app.samples.iter();
    for section in &sections {
        rows.push(Row::new(vec![Cell::from(Span::styled(
            section.title.clone(),
            p.style(Role::Heading),
        ))]));
        for row in &section.rows {
            let sample = samples.next();
            let trend = sample
                .and_then(|s| Some((s, app.trend(s)?)))
                .map(|(s, t)| spark(t.iter().copied(), trend_width, range_for(s.descriptor.unit)))
                .unwrap_or_default();
            rows.push(Row::new(vec![
                Cell::from(format!("  {}", row.label)),
                Cell::from(
                    Line::from(row.cells[0].text.clone())
                        .alignment(Alignment::Right)
                        .style(p.style(row.cells[0].role)),
                ),
                Cell::from(Span::styled(trend, p.style(Role::Accent))),
            ]));
        }
    }
    let visible = area.height.saturating_sub(3) as usize;
    let offset = app.scroll.min(rows.len().saturating_sub(visible));
    let total = rows.len();
    let rows: Vec<Row> = rows.into_iter().skip(offset).collect();
    let title = if total > visible {
        format!(
            "Sensors {}-{} of {total}",
            offset + 1,
            (offset + visible).min(total)
        )
    } else {
        "Sensors".to_owned()
    };
    let table = Table::new(
        rows,
        [
            Constraint::Min(24),
            Constraint::Length(14),
            Constraint::Length(trend_width as u16 + 1),
        ],
    )
    .header(Row::new(vec!["Sensor", "Value", "Trend"]).style(p.style(Role::Muted)))
    .column_spacing(2)
    .block(block(&title, p).padding(Padding::horizontal(1)));
    f.render_widget(table, area);
}

fn draw_gpus(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    if app.system.gpus.is_empty() {
        let reason = format::support(app.capabilities.support(surtr_model::Feature::GpuIdentity)).0;
        let text = Paragraph::new(format!(" No GPU found: {reason}."))
            .style(p.style(Role::Missing))
            .wrap(Wrap { trim: false })
            .block(block("GPU", p));
        f.render_widget(text, area);
        return;
    }
    let heights: Vec<Constraint> = app
        .system
        .gpus
        .iter()
        .map(|g| {
            let procs = app
                .processes
                .iter()
                .filter(|x| x.device == g.device)
                .count();
            Constraint::Length(8 + procs.min(5) as u16)
        })
        .chain(std::iter::once(Constraint::Min(0)))
        .collect();
    let areas = Layout::vertical(heights).split(area);
    for (gpu, &area) in app.system.gpus.iter().zip(areas.iter()) {
        let name = gpu
            .name
            .as_ref()
            .map_or("unknown GPU", |n| n.value.as_str());
        let title = format!("{} · {name}", gpu.device);
        let outer = block(&title, p);
        let inner = outer.inner(area);
        f.render_widget(outer, area);

        let mine = |label: &str| {
            app.samples
                .iter()
                .find(|s| s.descriptor.device == gpu.device && s.descriptor.label == label)
                .and_then(|s| s.value)
        };
        let [load, vram, power, info, throttle, procs] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);

        let gauge = |label: String, ratio: Option<f64>, role: Role| {
            LineGauge::default()
                .ratio(ratio.unwrap_or(0.0).clamp(0.0, 1.0))
                .label(Span::styled(format!(" {label:<24}"), p.style(Role::Text)))
                .filled_style(p.style(role))
                .unfilled_style(p.style(Role::Muted))
        };
        let gpu_load = mine("GPU load");
        f.render_widget(
            gauge(
                format!(
                    "Load    {}",
                    gpu_load.map_or(format::MISSING.into(), |v| format::value(Unit::Percent, v))
                ),
                gpu_load.map(|v| v / 100.0),
                Role::Accent,
            ),
            load,
        );
        let used = mine("VRAM used");
        let total = gpu.vram_bytes.as_ref().map(|t| t.value as f64);
        let vram_text = match (used, total) {
            (Some(u), Some(t)) => format!(
                "VRAM    {} / {}",
                format::value(Unit::Byte, u),
                format::value(Unit::Byte, t)
            ),
            (Some(u), None) => format!("VRAM    {}", format::value(Unit::Byte, u)),
            _ => format!("VRAM    {}", format::MISSING),
        };
        f.render_widget(
            gauge(vram_text, used.zip(total).map(|(u, t)| u / t), Role::Accent),
            vram,
        );
        let watts = mine("Board power");
        let limit = mine("Power limit");
        let power_text = match (watts, limit) {
            (Some(w), Some(l)) => format!("Power   {:.0} / {:.0} W", w, l),
            (Some(w), None) => format!("Power   {}", format::value(Unit::Watt, w)),
            _ => format!("Power   {}", format::MISSING),
        };
        let near_limit = watts.zip(limit).is_some_and(|(w, l)| w >= 0.95 * l);
        f.render_widget(
            gauge(
                power_text,
                watts.zip(limit).map(|(w, l)| w / l),
                if near_limit { Role::Warn } else { Role::Accent },
            ),
            power,
        );

        // Temperature (colored by health), clocks.
        let mut spans = vec![Span::raw(" ")];
        if let Some(s) = app.samples.iter().find(|s| {
            s.descriptor.device == gpu.device && s.descriptor.kind == SensorKind::Temperature
        }) && let Some(v) = s.value
        {
            let role = surtr_view::health::level(&s.descriptor, v).map_or(Role::Text, |l| l.role());
            spans.push(Span::styled(format::value(Unit::Celsius, v), p.style(role)));
            spans.push(Span::raw("   "));
        }
        for label in ["Graphics clock", "Memory clock"] {
            if let Some(v) = mine(label) {
                spans.push(Span::styled(format!("{label} "), p.style(Role::Muted)));
                spans.push(Span::raw(format!(
                    "{}   ",
                    format::value(Unit::Megahertz, v)
                )));
            }
        }
        f.render_widget(Line::from(spans), info);

        // Throttle reasons.
        let mut spans = vec![Span::styled(" Limited by  ", p.style(Role::Muted))];
        match app.throttle.iter().find(|t| t.device == gpu.device) {
            None => spans.push(Span::styled(format::MISSING, p.style(Role::Missing))),
            Some(t) if t.reasons.is_empty() => {
                spans.push(Span::styled("nothing", p.style(Role::Ok)));
            }
            Some(t) => {
                for (i, &r) in t.reasons.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::raw(", "));
                    }
                    spans.push(Span::styled(
                        format::throttle(r),
                        p.style(format::throttle_role(r)),
                    ));
                }
            }
        }
        f.render_widget(Line::from(spans), throttle);

        // Processes.
        let rows: Vec<Row> = app
            .processes
            .iter()
            .filter(|x| x.device == gpu.device)
            .take(5)
            .map(|x| {
                let memory = x
                    .used_memory_bytes
                    .as_ref()
                    .map_or("n/a".to_owned(), |m| format::bytes(m.value));
                Row::new(vec![
                    Cell::from(format!(" {}", x.pid)),
                    Cell::from(Span::styled(
                        format::process_kind(x.kind),
                        p.style(Role::Muted),
                    )),
                    Cell::from(Line::from(memory).alignment(Alignment::Right)),
                    Cell::from(x.name.clone().unwrap_or_else(|| "?".to_owned())),
                ])
            })
            .collect();
        if rows.is_empty() {
            f.render_widget(
                Span::styled(" No processes on this GPU.", p.style(Role::Muted)),
                procs,
            );
        } else {
            let table = Table::new(
                rows,
                [
                    Constraint::Length(9),
                    Constraint::Length(9),
                    Constraint::Length(10),
                    Constraint::Min(0),
                ],
            )
            .header(
                Row::new(vec![" PID", "Type", "    Memory", "Name"]).style(p.style(Role::Muted)),
            );
            f.render_widget(table, procs);
        }
    }
}

fn draw_capabilities(f: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let rows: Vec<Row> = capability_rows(&app.capabilities)
        .into_iter()
        .skip(app.scroll)
        .map(|row| {
            // A remedy, when there is one, goes on the line under the status.
            let mut status = vec![Line::from(Span::styled(
                row.cells[0].text.clone(),
                p.style(row.cells[0].role),
            ))];
            if let Some(fix) = row.cells.get(1) {
                status.push(Line::from(Span::styled(
                    format!("→ {}", fix.text),
                    p.style(fix.role),
                )));
            }
            let height = status.len() as u16;
            Row::new(vec![
                Cell::from(format!(" {}", row.label)),
                Cell::from(status),
            ])
            .height(height)
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(24), Constraint::Min(0)])
        .header(Row::new(vec![" Feature", "Status"]).style(p.style(Role::Muted)))
        .block(block("Capabilities", p));
    f.render_widget(table, area);
}

fn draw_help(f: &mut Frame, area: Rect, p: &Palette) {
    let keys = [
        ("1-4", "switch view"),
        ("←/→, Tab", "next or previous view"),
        ("↑/↓, j/k", "scroll"),
        ("PgUp/PgDn", "scroll by a page"),
        ("Home", "back to the top"),
        ("mouse", "click a tab, wheel to scroll"),
        ("?", "this help"),
        ("q, Esc", "quit"),
    ];
    let width = 44.min(area.width);
    let height = (keys.len() as u16 + 4).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let lines: Vec<Line> = keys
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<12}"), p.style(Role::Accent)),
                Span::raw(*what),
            ])
        })
        .chain([
            Line::raw(""),
            Line::styled(" Any key closes this.", p.style(Role::Muted)),
        ])
        .collect();
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(block("Keys", p))
            .style(Style::new()),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_scales() {
        assert_eq!(spark([0.0, 50.0, 100.0], 10, Some((0.0, 100.0))), "▁▅█");
        assert_eq!(spark([5.0, 5.0], 10, None), "▁▁");
        assert_eq!(spark((0..30).map(f64::from), 4, None), "▁▃▆█");
        assert_eq!(spark([], 4, None), "");
    }

    #[test]
    fn tab_hit_testing() {
        // " SURTR " + " " then " 1 Overview " starts at column 8.
        assert_eq!(tab_at(7), None);
        assert_eq!(tab_at(8), Some(Tab::Overview));
        assert_eq!(tab_at(8 + 12), Some(Tab::Sensors));
        assert_eq!(tab_at(200), None);
    }
}
