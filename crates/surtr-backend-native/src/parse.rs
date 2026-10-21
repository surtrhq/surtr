//! Pure parsing helpers, compiled on every platform so their tests run in CI
//! everywhere.

use surtr_model::{Arch, CpuVendor};

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

/// `MemTotal` from `/proc/meminfo`, in bytes.
pub fn meminfo_total_bytes(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let mut parts = line["MemTotal:".len()..].split_whitespace();
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
        assert_eq!(meminfo_total_bytes(text), Some(16_314_748 * 1024));
        assert_eq!(meminfo_total_bytes("MemFree: 1 kB"), None);
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
}
