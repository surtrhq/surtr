//! Pure parsing helpers, compiled on every platform so their tests run in CI
//! everywhere.

use surtr_model::{Arch, CpuVendor, SensorKind, Unit};

/// `(name, version)` from an os-release file: `PRETTY_NAME` (or `NAME`) and
/// `VERSION_ID`.
pub fn os_release(text: &str) -> (Option<String>, Option<String>) {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key)
                .then(|| v.trim().trim_matches(['"', '\'']).to_owned())
                .filter(|v| !v.is_empty())
        })
    };
    (
        value("PRETTY_NAME").or_else(|| value("NAME")),
        value("VERSION_ID"),
    )
}

/// A `/proc/meminfo` value such as `MemTotal`, in bytes.
pub fn meminfo_bytes(text: &str, key: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|l| l.split_once(':').is_some_and(|(k, _)| k == key))?;
    let mut parts = line[key.len() + 1..].split_whitespace();
    let value: u64 = parts.next()?.parse().ok()?;
    match parts.next() {
        Some("kB") => value.checked_mul(1024),
        None => Some(value),
        Some(_) => None,
    }
}

/// CPU numbers from a sysfs list such as `"0-3,8,10-11"`.
pub fn cpu_list(text: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                    cpus.extend(a..=b);
                }
            }
            None => cpus.extend(part.trim().parse::<u32>().ok()),
        }
    }
    cpus
}

/// First value of `key` in `/proc/cpuinfo`.
pub fn cpuinfo_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key)
            .then(|| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// Vendor from a CPU vendor string (CPUID vendor id or OS-reported name).
pub fn cpu_vendor(id: &str) -> CpuVendor {
    let id = id.trim();
    let lower = id.to_ascii_lowercase();
    match id {
        "GenuineIntel" => CpuVendor::Intel,
        "AuthenticAMD" => CpuVendor::Amd,
        _ if lower.contains("qualcomm") => CpuVendor::Qualcomm,
        _ if lower.starts_with("apple") => CpuVendor::Apple,
        _ if lower == "arm" || lower.starts_with("arm ") || lower.starts_with("arm,") => {
            CpuVendor::Arm
        }
        _ => CpuVendor::Other(id.to_owned()),
    }
}

/// Vendor from the Arm "CPU implementer" code in `/proc/cpuinfo`, such as `0x41`.
pub fn arm_implementer(code: &str) -> Option<CpuVendor> {
    let code = u32::from_str_radix(code.trim().trim_start_matches("0x"), 16).ok()?;
    Some(match code {
        0x41 => CpuVendor::Arm,
        0x51 => CpuVendor::Qualcomm,
        0x61 => CpuVendor::Apple,
        0x42 => CpuVendor::Other("Broadcom".to_owned()),
        0x43 => CpuVendor::Other("Cavium".to_owned()),
        0x48 => CpuVendor::Other("HiSilicon".to_owned()),
        0x4e => CpuVendor::Other("NVIDIA".to_owned()),
        0xc0 => CpuVendor::Other("Ampere".to_owned()),
        _ => CpuVendor::Other(format!("Arm implementer 0x{code:02x}")),
    })
}

/// The architecture Rust was built for, as a model value.
pub fn build_arch() -> Arch {
    match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => Arch::Other(other.to_owned()),
    }
}

/// Windows 11 still reports "Windows 10 ..." as its registry `ProductName`;
/// build 22000 and later is Windows 11.
pub fn windows_product_name(product: &str, build: Option<u32>) -> String {
    match (product.strip_prefix("Windows 10"), build) {
        (Some(rest), Some(b)) if b >= 22000 => format!("Windows 11{rest}"),
        _ => product.to_owned(),
    }
}

/// A frequency in kHz (sysfs cpufreq) as MHz.
pub fn khz_to_mhz(text: &str) -> Option<f64> {
    let khz: u64 = text.trim().parse().ok()?;
    (khz > 0).then(|| khz as f64 / 1000.0)
}

/// Processor masks for one topology record: `(group, mask)` pairs.
pub type GroupMasks = Vec<(u16, u64)>;

/// `(cores, logical processors)` per package, from Windows processor-group
/// records. A core belongs to the package whose masks overlap it. Without
/// package records, everything counts as one package.
pub fn group_topology(packages: &[GroupMasks], cores: &[GroupMasks]) -> Vec<(u32, u32)> {
    let overlaps = |a: &GroupMasks, b: &GroupMasks| {
        a.iter()
            .any(|(ga, ma)| b.iter().any(|(gb, mb)| ga == gb && ma & mb != 0))
    };
    let logical = |masks: &[GroupMasks]| {
        masks
            .iter()
            .flatten()
            .map(|(_, m)| m.count_ones())
            .sum::<u32>()
    };
    if packages.is_empty() {
        return vec![(count(cores.len()), logical(cores))];
    }
    packages
        .iter()
        .map(|package| {
            let own: Vec<GroupMasks> = cores
                .iter()
                .filter(|core| overlaps(package, core))
                .cloned()
                .collect();
            (count(own.len()), logical(std::slice::from_ref(package)))
        })
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Busy and total time of one CPU line in `/proc/stat`, in clock ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    /// Time not idle.
    pub busy: u64,
    /// All time.
    pub total: u64,
}

/// `(cpu number, times)` for each `cpu` line of `/proc/stat`; `None` is the
/// aggregate line.
pub fn proc_stat(text: &str) -> Vec<(Option<u32>, CpuTimes)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let rest = parts.next()?.strip_prefix("cpu")?;
            let cpu = if rest.is_empty() {
                None
            } else {
                Some(rest.parse().ok()?)
            };
            // user nice system idle iowait irq softirq steal (guest time is
            // already counted in user and nice).
            let v: Vec<u64> = parts.take(8).map(|p| p.parse().unwrap_or(0)).collect();
            if v.len() < 4 {
                return None;
            }
            let total: u64 = v.iter().sum();
            let idle = v[3] + v.get(4).copied().unwrap_or(0);
            Some((
                cpu,
                CpuTimes {
                    busy: total.saturating_sub(idle),
                    total,
                },
            ))
        })
        .collect()
}

/// Load in percent between two samples, or `None` if no time passed.
pub fn load_percent(prev: CpuTimes, cur: CpuTimes) -> Option<f64> {
    let total = cur.total.checked_sub(prev.total).filter(|&t| t > 0)?;
    let busy = cur.busy.saturating_sub(prev.busy).min(total);
    Some(busy as f64 * 100.0 / total as f64)
}

/// A hwmon value file such as `temp1_input`: what it measures, its unit, the
/// divisor from the file's integer to that unit, its type prefix and channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HwmonChannel {
    /// What it measures.
    pub kind: SensorKind,
    /// Unit after scaling.
    pub unit: Unit,
    /// Divides the raw integer into `unit` (for example 1000 for milli-units).
    pub divisor: f64,
    /// `temp`, `in`, `fan`, `power` or `curr`.
    pub prefix: &'static str,
    /// Channel number from the file name.
    pub channel: u32,
}

/// Parses a hwmon file name; only `*_input` files (and `power*_average`) count.
pub fn hwmon_channel(file: &str) -> Option<HwmonChannel> {
    const TYPES: [(&str, SensorKind, Unit, f64); 5] = [
        ("temp", SensorKind::Temperature, Unit::Celsius, 1e3),
        ("in", SensorKind::Voltage, Unit::Volt, 1e3),
        ("fan", SensorKind::Fan, Unit::Rpm, 1.0),
        ("power", SensorKind::Power, Unit::Watt, 1e6),
        ("curr", SensorKind::Current, Unit::Ampere, 1e3),
    ];
    let (name, suffix) = file.split_once('_')?;
    for (prefix, kind, unit, divisor) in TYPES {
        let Some(channel) = name.strip_prefix(prefix) else {
            continue;
        };
        let Ok(channel) = channel.parse() else {
            continue;
        };
        let wanted = suffix == "input" || (prefix == "power" && suffix == "average");
        return wanted.then_some(HwmonChannel {
            kind,
            unit,
            divisor,
            prefix,
            channel,
        });
    }
    None
}

/// The PCI vendor id in a Windows hardware id such as
/// `PCI\VEN_10DE&DEV_25B9&SUBSYS_0A821028`.
pub fn pci_vendor(hardware_id: &str) -> Option<u16> {
    let upper = hardware_id.to_ascii_uppercase();
    let start = upper.find("VEN_")? + 4;
    u16::from_str_radix(upper.get(start..start + 4)?, 16).ok()
}

/// A hex id from sysfs such as `0x10de`.
pub fn hex_id(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// A Windows PDH "Processor Information" instance name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PdhInstance {
    /// `_Total`: the whole system.
    Total,
    /// `G,_Total`: one processor group.
    GroupTotal,
    /// `G,N`: logical processor `N` in group `G`.
    Thread {
        /// Processor group.
        group: u16,
        /// Number inside the group.
        index: u32,
    },
}

/// Parses a PDH processor instance name such as `"0,5"` or `"_Total"`.
pub fn pdh_instance(name: &str) -> Option<PdhInstance> {
    if name == "_Total" {
        return Some(PdhInstance::Total);
    }
    let (group, rest) = name.split_once(',')?;
    let group = group.trim().parse().ok()?;
    if rest == "_Total" {
        return Some(PdhInstance::GroupTotal);
    }
    Some(PdhInstance::Thread {
        group,
        index: rest.trim().parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_prefers_pretty_name() {
        let text = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(
            os_release(text),
            (Some("Ubuntu 24.04.1 LTS".into()), Some("24.04".into()))
        );
        assert_eq!(
            os_release("NAME=Arch Linux\n"),
            (Some("Arch Linux".into()), None)
        );
        assert_eq!(os_release(""), (None, None));
    }

    #[test]
    fn meminfo_total_in_bytes() {
        let text = "MemTotal:       16314748 kB\nMemFree:         1234 kB\n";
        assert_eq!(meminfo_bytes(text, "MemTotal"), Some(16_314_748 * 1024));
        assert_eq!(meminfo_bytes("MemFree: 1 kB", "MemTotal"), None);
    }

    #[test]
    fn cpu_lists_expand_ranges() {
        assert_eq!(cpu_list("0-3,8,10-11\n"), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(cpu_list("0"), vec![0]);
        assert!(cpu_list("").is_empty());
    }

    #[test]
    fn cpuinfo_first_value() {
        let text = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Test CPU\n\nprocessor\t: 1\nmodel name\t: AMD Test CPU\n";
        assert_eq!(
            cpuinfo_value(text, "model name").as_deref(),
            Some("AMD Test CPU")
        );
        assert_eq!(cpuinfo_value(text, "Hardware"), None);
    }

    #[test]
    fn vendors_from_strings_and_codes() {
        assert_eq!(cpu_vendor("GenuineIntel"), CpuVendor::Intel);
        assert_eq!(cpu_vendor("AuthenticAMD"), CpuVendor::Amd);
        assert_eq!(cpu_vendor("Qualcomm Technologies Inc"), CpuVendor::Qualcomm);
        assert_eq!(cpu_vendor("Apple"), CpuVendor::Apple);
        assert_eq!(
            cpu_vendor("HygonGenuine"),
            CpuVendor::Other("HygonGenuine".into())
        );
        assert_eq!(arm_implementer("0x41"), Some(CpuVendor::Arm));
        assert_eq!(arm_implementer("0x61"), Some(CpuVendor::Apple));
        assert_eq!(arm_implementer("zz"), None);
    }

    #[test]
    fn windows_11_is_named_by_build() {
        assert_eq!(
            windows_product_name("Windows 10 Pro", Some(26200)),
            "Windows 11 Pro"
        );
        assert_eq!(
            windows_product_name("Windows 10 Pro", Some(19045)),
            "Windows 10 Pro"
        );
        assert_eq!(
            windows_product_name("Windows 10 Pro", None),
            "Windows 10 Pro"
        );
        assert_eq!(
            windows_product_name("Windows Server 2022 Datacenter", Some(20348)),
            "Windows Server 2022 Datacenter"
        );
    }

    #[test]
    fn khz_values() {
        assert_eq!(khz_to_mhz("4500000\n"), Some(4500.0));
        assert_eq!(khz_to_mhz("0"), None);
        assert_eq!(khz_to_mhz("n/a"), None);
    }

    #[test]
    fn topology_single_package_with_smt() {
        // 4 cores, 2 threads each, in group 0.
        let cores: Vec<GroupMasks> = (0..4).map(|i| vec![(0, 0b11 << (2 * i))]).collect();
        let packages = vec![vec![(0, 0xff)]];
        assert_eq!(group_topology(&packages, &cores), vec![(4, 8)]);
    }

    #[test]
    fn topology_two_packages() {
        let cores: Vec<GroupMasks> = (0..8).map(|i| vec![(0, 1 << i)]).collect();
        let packages = vec![vec![(0, 0x0f)], vec![(0, 0xf0)]];
        assert_eq!(group_topology(&packages, &cores), vec![(4, 4), (4, 4)]);
    }

    #[test]
    fn topology_spanning_processor_groups() {
        let cores = vec![vec![(0, 1)], vec![(1, 1)]];
        let packages = vec![vec![(0, 1), (1, 1)]];
        assert_eq!(group_topology(&packages, &cores), vec![(2, 2)]);
    }

    #[test]
    fn topology_without_package_records() {
        let cores = vec![vec![(0, 0b11)], vec![(0, 0b1100)]];
        assert_eq!(group_topology(&[], &cores), vec![(2, 4)]);
    }

    #[test]
    fn proc_stat_lines() {
        let text = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\nintr 1 2 3\n";
        let stats = proc_stat(text);
        assert_eq!(stats.len(), 2);
        assert_eq!(
            stats[0],
            (
                None,
                CpuTimes {
                    busy: 150,
                    total: 1000
                }
            )
        );
        assert_eq!(stats[1].0, Some(0));
    }

    #[test]
    fn load_between_samples() {
        let a = CpuTimes {
            busy: 100,
            total: 1000,
        };
        let b = CpuTimes {
            busy: 150,
            total: 1100,
        };
        assert_eq!(load_percent(a, b), Some(50.0));
        assert_eq!(load_percent(a, a), None);
        assert_eq!(load_percent(b, a), None);
    }

    #[test]
    fn meminfo_values() {
        let text = "MemTotal:       16314748 kB\nMemAvailable:    8000000 kB\n";
        assert_eq!(meminfo_bytes(text, "MemAvailable"), Some(8_000_000 * 1024));
        assert_eq!(meminfo_bytes(text, "Mem"), None);
    }

    #[test]
    fn hwmon_file_names() {
        let t = hwmon_channel("temp1_input").unwrap();
        assert_eq!(
            (t.kind, t.unit, t.channel),
            (SensorKind::Temperature, Unit::Celsius, 1)
        );
        assert_eq!(hwmon_channel("in0_input").unwrap().unit, Unit::Volt);
        assert_eq!(hwmon_channel("power1_average").unwrap().unit, Unit::Watt);
        assert_eq!(hwmon_channel("fan2_input").unwrap().channel, 2);
        assert_eq!(hwmon_channel("temp1_label"), None);
        assert_eq!(hwmon_channel("temp1_max"), None);
        assert_eq!(hwmon_channel("intrusion0_alarm"), None);
        assert_eq!(hwmon_channel("name"), None);
    }

    #[test]
    fn pci_ids() {
        assert_eq!(
            pci_vendor(r"PCI\VEN_10DE&DEV_25B9&SUBSYS_0A821028&REV_A1"),
            Some(0x10de)
        );
        assert_eq!(pci_vendor(r"pci\ven_8086&dev_46a6"), Some(0x8086));
        assert_eq!(pci_vendor(r"ROOT\BasicDisplay"), None);
        assert_eq!(hex_id("0x030000\n"), Some(0x030000));
        assert_eq!(hex_id("zz"), None);
    }

    #[test]
    fn pdh_instance_names() {
        assert_eq!(pdh_instance("_Total"), Some(PdhInstance::Total));
        assert_eq!(pdh_instance("0,_Total"), Some(PdhInstance::GroupTotal));
        assert_eq!(
            pdh_instance("1,5"),
            Some(PdhInstance::Thread { group: 1, index: 5 })
        );
        assert_eq!(pdh_instance("bogus"), None);
    }
}
