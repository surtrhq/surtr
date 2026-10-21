//! Linux: procfs and sysfs files, no root needed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use surtr_model::{CpuInfo, MemoryInfo, OsInfo, Source, SystemInfo};

use crate::parse;

pub const ID: &str = "linux";
pub const SUPPORTED: bool = true;

const CPU_DIR: &str = "/sys/devices/system/cpu";

fn read(path: &str) -> Option<String> {
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
        read(&format!("{CPU_DIR}/cpu0/cpufreq/base_frequency")).and_then(|t| parse::khz_to_mhz(&t));
    let max = read(&format!("{CPU_DIR}/cpu0/cpufreq/cpuinfo_max_freq"))
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

/// Package id -> (distinct cores, logical processors), for online CPUs.
fn packages() -> BTreeMap<i64, (BTreeSet<String>, u32)> {
    let mut packages: BTreeMap<i64, (BTreeSet<String>, u32)> = BTreeMap::new();
    let Some(online) = read(&format!("{CPU_DIR}/online")) else {
        return packages;
    };
    for cpu in parse::cpu_list(&online) {
        let topo = format!("{CPU_DIR}/cpu{cpu}/topology");
        let package = read(&format!("{topo}/physical_package_id"))
            .and_then(|t| t.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let Some(core) = read(&format!("{topo}/core_cpus_list"))
            .or_else(|| read(&format!("{topo}/thread_siblings_list")))
        else {
            continue;
        };
        let slot = packages.entry(package).or_default();
        slot.0.insert(core.trim().to_owned());
        slot.1 += 1;
    }
    packages
}

fn memory() -> MemoryInfo {
    let total = read("/proc/meminfo").and_then(|t| parse::meminfo_total_bytes(&t));
    MemoryInfo {
        total_bytes: total.and_then(|t| Source::new(ID, "procfs:meminfo").tag(t)),
        modules: Vec::new(),
    }
}
