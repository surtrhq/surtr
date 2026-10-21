//! View models: what each screen shows, as titled sections of labelled rows.

use surtr_core::Sample;
use surtr_model::{Capabilities, Feature, Field, GpuProcess, SystemInfo, ThrottleState};

use crate::{
    Role,
    format::{self, MISSING},
    health,
};

/// One piece of text and what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The text.
    pub text: String,
    /// Its meaning, for coloring.
    pub role: Role,
}

impl Cell {
    /// A cell with a role.
    pub fn new(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }

    /// A field's value as text, or the missing marker.
    pub fn field<T>(field: &Field<T>, fmt: impl Fn(&T) -> String) -> Self {
        match field {
            Some(f) => Self::new(fmt(&f.value), Role::Text),
            None => Self::new(MISSING, Role::Missing),
        }
    }
}

/// A labelled row of one or more cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What the row is.
    pub label: String,
    /// Its values.
    pub cells: Vec<Cell>,
}

impl Row {
    fn new(label: impl Into<String>, cell: Cell) -> Self {
        Self {
            label: label.into(),
            cells: vec![cell],
        }
    }
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Title, for example "CPU 0" or a device name.
    pub title: String,
    /// Rows in display order.
    pub rows: Vec<Row>,
}

/// OS, each CPU, memory and each GPU.
pub fn system_sections(system: &SystemInfo) -> Vec<Section> {
    let text = |f: &Field<String>| Cell::field(f, Clone::clone);
    let count = |f: &Field<u32>| Cell::field(f, u32::to_string);
    let mhz = |f: &Field<f64>| Cell::field(f, |v| format::value(surtr_model::Unit::Megahertz, *v));
    let bytes = |f: &Field<u64>| Cell::field(f, |b| format::bytes(*b));

    let mut os = vec![
        Row::new("Name", text(&system.os.name)),
        Row::new("Version", text(&system.os.version)),
    ];
    if system.os.kernel.is_some() {
        os.push(Row::new("Kernel", text(&system.os.kernel)));
    }
    os.push(Row::new(
        "Architecture",
        Cell::field(&system.os.arch, format::arch),
    ));
    let mut out = vec![Section {
        title: "OS".to_owned(),
        rows: os,
    }];

    for (i, cpu) in system.cpus.iter().enumerate() {
        out.push(Section {
            title: format!("CPU {i}"),
            rows: vec![
                Row::new("Name", text(&cpu.brand)),
                Row::new("Vendor", Cell::field(&cpu.vendor, format::cpu_vendor)),
                Row::new("Cores", count(&cpu.physical_cores)),
                Row::new("Threads", count(&cpu.logical_cores)),
                Row::new("Base clock", mhz(&cpu.base_clock_mhz)),
                Row::new("Max clock", mhz(&cpu.max_clock_mhz)),
            ],
        });
    }

    out.push(Section {
        title: "Memory".to_owned(),
        rows: vec![Row::new("Total", bytes(&system.memory.total_bytes))],
    });

    for (i, gpu) in system.gpus.iter().enumerate() {
        out.push(Section {
            title: format!("GPU {i}"),
            rows: vec![
                Row::new("Name", text(&gpu.name)),
                Row::new("Vendor", Cell::field(&gpu.vendor, format::gpu_vendor)),
                Row::new("Memory", bytes(&gpu.vram_bytes)),
                Row::new("Driver", text(&gpu.driver_version)),
            ],
        });
    }
    out
}

/// Every feature with its support status, including ones no provider
/// mentions.
pub fn capability_rows(capabilities: &Capabilities) -> Vec<Row> {
    Feature::ALL
        .iter()
        .map(|&feature| {
            let (text, role) = format::support(capabilities.support(feature));
            Row::new(format::feature(feature), Cell::new(text, role))
        })
        .collect()
}

/// Sensors grouped by device, in provider order; temperatures are colored by
/// health level.
pub fn sensor_sections(samples: &[Sample]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for s in samples {
        let d = &s.descriptor;
        let cell = match s.value {
            Some(v) => {
                let role = health::level(d, v).map_or(Role::Text, |l| l.role());
                Cell::new(format::value(d.unit, v), role)
            }
            None => Cell::new(MISSING, Role::Missing),
        };
        let row = Row::new(d.label.clone(), cell);
        match out.last_mut() {
            Some(section) if section.title == d.device => section.rows.push(row),
            _ => out.push(Section {
                title: d.device.clone(),
                rows: vec![row],
            }),
        }
    }
    out
}

/// GPU processes grouped by device. Each row is labelled with the PID and has
/// cells for kind, memory and name.
pub fn process_rows(processes: &[GpuProcess]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for p in processes {
        let memory = p.used_memory_bytes.as_ref().map_or_else(
            || Cell::new("n/a", Role::Missing),
            |m| Cell::new(format::bytes(m.value), Role::Text),
        );
        let name = p.name.as_deref().map_or_else(
            || Cell::new("?", Role::Missing),
            |n| Cell::new(n, Role::Text),
        );
        let row = Row {
            label: p.pid.to_string(),
            cells: vec![
                Cell::new(format::process_kind(p.kind), Role::Muted),
                memory,
                name,
            ],
        };
        match out.last_mut() {
            Some(section) if section.title == p.device => section.rows.push(row),
            _ => out.push(Section {
                title: p.device.clone(),
                rows: vec![row],
            }),
        }
    }
    out
}

/// One row per device with a cell per active throttle reason.
pub fn throttle_rows(states: &[ThrottleState]) -> Vec<Row> {
    states
        .iter()
        .map(|s| Row {
            label: s.device.clone(),
            cells: if s.reasons.is_empty() {
                vec![Cell::new("none (running unrestricted)", Role::Ok)]
            } else {
                s.reasons
                    .iter()
                    .map(|&r| Cell::new(format::throttle(r), format::throttle_role(r)))
                    .collect()
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_backend::{Provider, mock::MockProvider};

    fn samples(mock: &MockProvider) -> Vec<Sample> {
        let readings = mock.read_sensors().unwrap();
        mock.sensors()
            .unwrap()
            .into_iter()
            .map(|descriptor| {
                let r = readings.iter().find(|r| r.id == descriptor.id);
                Sample {
                    value: r.map(|r| r.value),
                    at: r.map(|r| r.at),
                    descriptor,
                }
            })
            .collect()
    }

    #[test]
    fn system_sections_cover_every_part() {
        let info = MockProvider::desktop().system_info().unwrap();
        let titles: Vec<_> = system_sections(&info)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(titles, ["OS", "CPU 0", "Memory", "GPU 0"]);
    }

    #[test]
    fn missing_values_are_marked() {
        let mut info = MockProvider::desktop().system_info().unwrap();
        info.cpus[0].max_clock_mhz = None;
        let cpu = &system_sections(&info)[1];
        let max = cpu.rows.iter().find(|r| r.label == "Max clock").unwrap();
        assert_eq!(max.cells[0], Cell::new(MISSING, Role::Missing));
    }

    #[test]
    fn sensors_group_by_device_and_color_temperatures() {
        let sections = sensor_sections(&samples(&MockProvider::desktop()));
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "cpu0");
        let temp = &sections[0].rows[0];
        assert_eq!(temp.cells[0].role, Role::Ok);
        let load = &sections[0].rows[2];
        assert_eq!(load.cells[0].role, Role::Text);
    }

    #[test]
    fn throttle_reasons_carry_severity() {
        let states = MockProvider::desktop().throttle_reasons().unwrap();
        let rows = throttle_rows(&states);
        assert_eq!(rows[0].cells[0], Cell::new("power limit", Role::Warn));
    }
}
