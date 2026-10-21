//! Linux: procfs and sysfs files, no root needed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use surtr_model::{
    Capabilities, CpuInfo, Feature, MemoryInfo, OsInfo, SensorDescriptor, SensorId, SensorKind,
    SensorReading, Source, Support, SystemInfo, Timestamp,
};

use crate::{
    descriptor,
    parse::{self, CpuTimes},
    reading,
};

pub const ID: &str = "linux";
pub const SUPPORTED: bool = true;

const CPU_DIR: &str = "/sys/devices/system/cpu";
const HWMON_DIR: &str = "/sys/class/hwmon";

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

pub fn system_info() -> SystemInfo {
    SystemInfo {
        os: os_info(),
        cpus: cpus(),
        memory: memory(),
        gpus: Vec::new(),
    }
}

fn os_info() -> OsInfo {
    let release = Source::new(ID, "os-release");
    let (name, version) = read("/etc/os-release")
        .or_else(|| read("/usr/lib/os-release"))
        .map(|t| parse::os_release(&t))
        .unwrap_or_default();
    let kernel = read("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    OsInfo {
        name: name.and_then(|n| release.tag(n)),
        version: version.and_then(|v| release.tag(v)),
        kernel: kernel.and_then(|k| Source::new(ID, "procfs").tag(k)),
        arch: Source::new(ID, "build-target").tag(parse::build_arch()),
    }
}

/// One entry per physical package, from sysfs topology; brand and vendor from
/// `/proc/cpuinfo`, clocks from cpufreq.
fn cpus() -> Vec<CpuInfo> {
    let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
    let procfs = Source::new(ID, "procfs:cpuinfo");
    let brand = parse::cpuinfo_value(&cpuinfo, "model name");
    let vendor = parse::cpuinfo_value(&cpuinfo, "vendor_id")
        .map(|v| parse::cpu_vendor(&v))
        .or_else(|| {
            parse::cpuinfo_value(&cpuinfo, "CPU implementer")
                .and_then(|c| parse::arm_implementer(&c))
        });

    let cpufreq = Source::new(ID, "sysfs:cpufreq");
    let base =
        read(format!("{CPU_DIR}/cpu0/cpufreq/base_frequency")).and_then(|t| parse::khz_to_mhz(&t));
    let max = read(format!("{CPU_DIR}/cpu0/cpufreq/cpuinfo_max_freq"))
        .and_then(|t| parse::khz_to_mhz(&t));

    let topology = Source::new(ID, "sysfs:topology");
    let entry = |cores: Option<u32>, logical: Option<u32>| CpuInfo {
        vendor: vendor.clone().and_then(|v| procfs.tag(v)),
        brand: brand.clone().and_then(|b| procfs.tag(b)),
        physical_cores: cores.and_then(|c| topology.tag(c)),
        logical_cores: logical.and_then(|l| topology.tag(l)),
        base_clock_mhz: base.and_then(|b| cpufreq.tag(b)),
        max_clock_mhz: max.and_then(|m| cpufreq.tag(m)),
    };

    let packages = packages();
    if packages.is_empty() {
        // No sysfs topology (some containers): count processors only.
        let logical = cpuinfo
            .lines()
            .filter(|l| l.starts_with("processor"))
            .count();
        let logical = u32::try_from(logical).ok().filter(|&n| n > 0);
        return vec![entry(None, logical)];
    }
    packages
        .into_values()
        .map(|(cores, logical)| {
            let cores = u32::try_from(cores.len()).ok();
            entry(cores, Some(logical))
        })
        .collect()
}

pub fn process_name(pid: u32) -> Option<String> {
    // `comm` is the executable name, truncated by the kernel to 15 bytes.
    read(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn online_cpus() -> Vec<u32> {
    read(format!("{CPU_DIR}/online"))
        .map(|t| parse::cpu_list(&t))
        .unwrap_or_default()
}

/// Package id -> (distinct cores, logical processors), for online CPUs.
fn packages() -> BTreeMap<i64, (BTreeSet<String>, u32)> {
    let mut packages: BTreeMap<i64, (BTreeSet<String>, u32)> = BTreeMap::new();
    for cpu in online_cpus() {
        let topo = format!("{CPU_DIR}/cpu{cpu}/topology");
        let package = read(format!("{topo}/physical_package_id"))
            .and_then(|t| t.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let Some(core) = read(format!("{topo}/core_cpus_list"))
            .or_else(|| read(format!("{topo}/thread_siblings_list")))
        else {
            continue;
        };
        let slot = packages.entry(package).or_default();
        slot.0.insert(core.trim().to_owned());
        slot.1 += 1;
    }
    packages
}

fn meminfo(key: &str) -> Option<u64> {
    read("/proc/meminfo").and_then(|t| parse::meminfo_bytes(&t, key))
}

fn memory() -> MemoryInfo {
    MemoryInfo {
        total_bytes: meminfo("MemTotal").and_then(|t| Source::new(ID, "procfs:meminfo").tag(t)),
        modules: Vec::new(),
    }
}

fn cur_freq_path(cpu: u32) -> String {
    format!("{CPU_DIR}/cpu{cpu}/cpufreq/scaling_cur_freq")
}

fn load_id(cpu: Option<u32>) -> SensorId {
    match cpu {
        None => SensorId::new("cpu/load"),
        Some(n) => SensorId::new(format!("cpu/thread{n}/load")),
    }
}

/// One hwmon value file.
#[derive(Debug, Clone)]
struct HwmonSensor {
    descriptor: SensorDescriptor,
    path: PathBuf,
    divisor: f64,
    /// Belongs to a CPU temperature driver (coretemp, k10temp, zenpower).
    cpu: bool,
}

/// Every readable value under a hwmon root such as `/sys/class/hwmon`.
/// Devices are named after the chip; CPU temperature drivers become `cpu0`,
/// `cpu1`, ... in sysfs order.
fn scan_hwmon(root: &Path) -> Vec<HwmonSensor> {
    let mut chips: Vec<(String, PathBuf)> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let name = read(dir.join("name"))?.trim().to_owned();
            Some((name, dir))
        })
        // GPU backends own their chips (surtr-backend-amd reads amdgpu).
        .filter(|(name, _)| name != "amdgpu")
        .collect();
    chips.sort();

    let source = Source::new(ID, "sysfs:hwmon");
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut sensors = Vec::new();
    for (chip, dir) in chips {
        let cpu = matches!(chip.as_str(), "coretemp" | "k10temp" | "zenpower");
        let family = if cpu { "cpu" } else { chip.as_str() }.to_owned();
        let n = seen.entry(family).or_insert(0);
        let device = match (cpu, *n) {
            (true, n) => format!("cpu{n}"),
            (false, 0) => chip.clone(),
            (false, n) => format!("{chip}-{n}"),
        };
        *n += 1;

        let mut files: Vec<(parse::HwmonChannel, String)> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().into_string().ok()?;
                Some((parse::hwmon_channel(&file)?, file))
            })
            .collect();
        // Sort by channel, `_input` before `_average`, then keep one per channel.
        files.sort_by(|a, b| {
            (a.0.prefix, a.0.channel, a.1.ends_with("_average")).cmp(&(
                b.0.prefix,
                b.0.channel,
                b.1.ends_with("_average"),
            ))
        });
        files.dedup_by(|b, a| a.0.prefix == b.0.prefix && a.0.channel == b.0.channel);

        for (channel, file) in files {
            let name = format!("{}{}", channel.prefix, channel.channel);
            let label = read(dir.join(format!("{name}_label")))
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| format!("{chip} {name}"));
            let mut descriptor = descriptor(
                &format!("hwmon/{device}/{name}"),
                channel.kind,
                &device,
                &label,
                &source,
            );
            descriptor.unit = channel.unit;
            sensors.push(HwmonSensor {
                descriptor,
                path: dir.join(file),
                divisor: channel.divisor,
                cpu,
            });
        }
    }
    sensors
}

/// Sensor state: what exists, and the previous `/proc/stat` sample for loads.
#[derive(Debug)]
pub struct State {
    online: Vec<u32>,
    clocks: Vec<u32>,
    hwmon: Vec<HwmonSensor>,
    prev_stat: Mutex<Vec<(Option<u32>, CpuTimes)>>,
}

impl State {
    pub fn new() -> Self {
        let online = online_cpus();
        let clocks = online
            .iter()
            .copied()
            .filter(|&cpu| Path::new(&cur_freq_path(cpu)).exists())
            .collect();
        // Prime the load counters so the first read has something to compare.
        let prev = read("/proc/stat")
            .map(|t| parse::proc_stat(&t))
            .unwrap_or_default();
        Self {
            online,
            clocks,
            hwmon: scan_hwmon(Path::new(HWMON_DIR)),
            prev_stat: Mutex::new(prev),
        }
    }

    pub fn capabilities(&self, caps: &mut Capabilities) {
        caps.set(Feature::CpuLoad, Support::Available);
        if !self.clocks.is_empty() {
            caps.set(Feature::CpuClock, Support::Available);
        }
        let has = |cpu: bool, kind: SensorKind| {
            self.hwmon
                .iter()
                .any(|s| s.cpu == cpu && s.descriptor.kind == kind)
        };
        caps.set(
            Feature::CpuTemperature,
            if has(true, SensorKind::Temperature) {
                Support::Available
            } else {
                Support::Unsupported {
                    reason: "no CPU temperature driver (coretemp, k10temp) found".to_owned(),
                }
            },
        );
        if has(false, SensorKind::Fan) || has(false, SensorKind::Voltage) {
            caps.set(Feature::BoardSensors, Support::Available);
        }
    }

    pub fn sensors(&self) -> Vec<SensorDescriptor> {
        let procfs = Source::new(ID, "procfs:stat");
        let cpufreq = Source::new(ID, "sysfs:cpufreq");
        let meminfo = Source::new(ID, "procfs:meminfo");
        let mut out = vec![descriptor(
            "cpu/load",
            SensorKind::Load,
            "cpu",
            "Total load",
            &procfs,
        )];
        for &cpu in &self.online {
            out.push(descriptor(
                &load_id(Some(cpu)).0,
                SensorKind::Load,
                "cpu",
                &format!("Thread {cpu} load"),
                &procfs,
            ));
        }
        for &cpu in &self.clocks {
            out.push(descriptor(
                &format!("cpu/thread{cpu}/clock"),
                SensorKind::Clock,
                "cpu",
                &format!("Thread {cpu} clock"),
                &cpufreq,
            ));
        }
        out.push(descriptor(
            "memory/used",
            SensorKind::Memory,
            "memory",
            "Used",
            &meminfo,
        ));
        out.extend(self.hwmon.iter().map(|s| s.descriptor.clone()));
        out
    }

    pub fn read(&self) -> Vec<SensorReading> {
        let at = Timestamp::now();
        let mut out = Vec::new();

        if let Some(text) = read("/proc/stat") {
            let cur = parse::proc_stat(&text);
            let mut prev = self
                .prev_stat
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for (cpu, times) in &cur {
                let before = prev.iter().find(|(c, _)| c == cpu).map(|(_, t)| *t);
                if let Some(load) = before.and_then(|p| parse::load_percent(p, *times)) {
                    out.push(reading(load_id(*cpu), load, at));
                }
            }
            *prev = cur;
        }

        for &cpu in &self.clocks {
            if let Some(mhz) = read(cur_freq_path(cpu)).and_then(|t| parse::khz_to_mhz(&t)) {
                out.push(reading(
                    SensorId::new(format!("cpu/thread{cpu}/clock")),
                    mhz,
                    at,
                ));
            }
        }

        if let (Some(total), Some(available)) = (meminfo("MemTotal"), meminfo("MemAvailable")) {
            out.push(reading(
                SensorId::new("memory/used"),
                total.saturating_sub(available) as f64,
                at,
            ));
        }

        out.extend(read_hwmon(&self.hwmon, at));
        out
    }
}

fn read_hwmon(sensors: &[HwmonSensor], at: Timestamp) -> Vec<SensorReading> {
    sensors
        .iter()
        .filter_map(|s| {
            // Unreadable channels (EIO, ENODATA) are skipped this time.
            let raw: f64 = read(&s.path)?.trim().parse().ok()?;
            Some(reading(s.descriptor.id.clone(), raw / s.divisor, at))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use surtr_model::Unit;

    /// A throwaway hwmon tree under the system temp dir.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("surtr-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn chip(&self, dir: &str, files: &[(&str, &str)]) {
            let dir = self.0.join(dir);
            fs::create_dir_all(&dir).unwrap();
            for (file, content) in files {
                fs::write(dir.join(file), content).unwrap();
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn hwmon_tree_becomes_sensors() {
        let fx = Fixture::new("hwmon");
        fx.chip(
            "hwmon0",
            &[
                ("name", "nct6798\n"),
                ("fan1_input", "1200\n"),
                ("in0_input", "1032\n"),
                ("in0_label", "Vcore\n"),
                ("temp1_max", "90000\n"),
            ],
        );
        fx.chip(
            "hwmon1",
            &[
                ("name", "k10temp\n"),
                ("temp1_input", "54250\n"),
                ("temp1_label", "Tctl\n"),
            ],
        );
        fx.chip(
            "hwmon2",
            &[
                ("name", "ina3221\n"),
                ("power1_average", "45000000\n"),
                ("power1_input", "47000000\n"),
            ],
        );
        // The AMD GPU backend owns this chip.
        fx.chip(
            "hwmon3",
            &[("name", "amdgpu\n"), ("temp1_input", "40000\n")],
        );

        let sensors = scan_hwmon(&fx.0);
        let ids: Vec<_> = sensors.iter().map(|s| s.descriptor.id.0.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hwmon/ina3221/power1",
                "hwmon/cpu0/temp1",
                "hwmon/nct6798/fan1",
                "hwmon/nct6798/in0",
            ]
        );
        let tctl = &sensors[1];
        assert!(tctl.cpu);
        assert_eq!(tctl.descriptor.label, "Tctl");
        assert_eq!(tctl.descriptor.unit, Unit::Celsius);
        assert_eq!(sensors[2].descriptor.label, "nct6798 fan1");

        let values: Vec<f64> = read_hwmon(&sensors, Timestamp(0))
            .iter()
            .map(|r| r.value)
            .collect();
        // power1_input wins over power1_average.
        assert_eq!(values, [47.0, 54.25, 1200.0, 1.032]);
    }

    #[test]
    fn missing_root_gives_no_sensors() {
        assert!(scan_hwmon(Path::new("/nonexistent/surtr")).is_empty());
    }
}
