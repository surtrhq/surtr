//! AMD GPU backend for Surtr through the Linux `amdgpu` kernel driver's sysfs
//! files. No library and no root needed.
//!
//! Devices are named `amd0`, `amd1`, ... in DRM card order. Per-process GPU
//! memory comes from DRM `fdinfo` (Linux 5.19 and later), for processes this
//! user may inspect. Windows support (ADLX) is a separate, optional backend
//! (plan, section 4).
//!
//! The code only touches files under the roots it is given, so tests run it
//! against fixture trees on every OS.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use surtr_backend::{Provider, ProviderError};
use surtr_model::{
    Capabilities, Feature, GpuInfo, GpuProcess, GpuProcessKind, GpuVendor, SensorDescriptor,
    SensorId, SensorKind, SensorReading, Source, Support, SystemInfo, ThrottleReason,
    ThrottleState, Timestamp, Unit,
};

const AMD_VENDOR: &str = "0x1002";
const PCI_IDS: [&str; 3] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn read_f64(path: &Path) -> Option<f64> {
    read(path)?.parse().ok()
}

/// The product name of PCI device `device` (for example `0x744c`) of vendor
/// AMD (`1002`) in a `pci.ids` database.
pub fn pci_name(pci_ids: &str, device: &str) -> Option<String> {
    let device = device.trim().trim_start_matches("0x").to_ascii_lowercase();
    let mut in_vendor = false;
    for line in pci_ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            in_vendor = line.starts_with("1002 ");
            continue;
        }
        if in_vendor && !line.starts_with("\t\t") {
            let (id, name) = line.trim_start().split_once(char::is_whitespace)?;
            if id.eq_ignore_ascii_case(&device) {
                return Some(name.trim().to_owned());
            }
        }
    }
    None
}

/// One DRM file descriptor's `fdinfo`, as far as Surtr uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdInfo {
    /// `drm-driver`.
    pub driver: String,
    /// `drm-pdev`: the PCI slot, for example `0000:03:00.0`.
    pub pdev: String,
    /// `drm-client-id`: several fds can share one client.
    pub client_id: Option<u64>,
    /// `drm-memory-vram`, in bytes.
    pub vram_bytes: Option<u64>,
}

/// Parses a DRM `fdinfo` file; `None` for non-DRM file descriptors.
pub fn parse_fdinfo(text: &str) -> Option<FdInfo> {
    let value = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    let vram = value("drm-memory-vram").and_then(|v| {
        let mut parts = v.split_whitespace();
        let n: u64 = parts.next()?.parse().ok()?;
        let unit = match parts.next() {
            None => 1,
            Some("KiB") => 1 << 10,
            Some("MiB") => 1 << 20,
            Some("GiB") => 1 << 30,
            Some(_) => return None,
        };
        n.checked_mul(unit)
    });
    Some(FdInfo {
        driver: value("drm-driver")?,
        pdev: value("drm-pdev")?,
        client_id: value("drm-client-id").and_then(|v| v.parse().ok()),
        vram_bytes: vram,
    })
}

/// The ASIC-independent throttle status from a `gpu_metrics` table, as the
/// kernel's `SMU_THROTTLER_*_BIT` mask.
///
/// Only tables v1.3 (dGPUs) and v2.2 to v2.4 (APUs) carry the field; for
/// those its offset is fixed by `kgd_pp_interface.h`. The kernel fills tables
/// with `0xFF` before the ASIC code writes them, so all ones means this GPU
/// doesn't report it.
pub fn indep_throttle_status(table: &[u8]) -> Option<u64> {
    let size = usize::from(u16::from_ne_bytes([*table.first()?, *table.get(1)?]));
    let offset = match (*table.get(2)?, *table.get(3)?) {
        (1, 3) => 112,
        (2, 2..=4) => 120,
        _ => return None,
    };
    if size < offset + 8 {
        return None;
    }
    let bits = u64::from_ne_bytes(table.get(offset..offset + 8)?.try_into().ok()?);
    (bits != u64::MAX).then_some(bits)
}

/// Decodes the kernel's ASIC-independent throttler bits
/// (`SMU_THROTTLER_*_BIT` in `amdgpu_smu.h`).
pub fn decode_throttle(bits: u64) -> Vec<ThrottleReason> {
    let mut reasons: Vec<ThrottleReason> = Vec::new();
    for bit in (0..64).filter(|b| bits & (1 << b) != 0) {
        let reason = match bit {
            // PPT0-3, SPL, FPPT, SPPT, SPPT_APU.
            0..=7 => ThrottleReason::PowerCap,
            // TDC (GFX, SOC, MEM, VDD, CVIP) and EDC (CPU, GFX).
            16..=22 => ThrottleReason::CurrentLimit,
            // GPU, core, memory, edge, hotspot, SoC, VR and liquid temperatures.
            32..=43 => ThrottleReason::SoftwareThermal,
            // VRHOT and PROCHOT signals.
            44..=47 => ThrottleReason::HardwareThermal,
            // APCC, PPM, FIT and bits newer than this list.
            _ => ThrottleReason::Other,
        };
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    reasons
}

fn label(text: &str) -> String {
    match text {
        "edge" => "Temperature (edge)".to_owned(),
        "junction" => "Temperature (hotspot)".to_owned(),
        "mem" => "Memory temperature".to_owned(),
        "sclk" => "Graphics clock".to_owned(),
        "mclk" => "Memory clock".to_owned(),
        "vddgfx" => "GPU voltage".to_owned(),
        "vddnb" => "SoC voltage".to_owned(),
        "PPT" => "Board power".to_owned(),
        other => other.to_owned(),
    }
}

fn throttle_bits(dir: &Path) -> Option<u64> {
    indep_throttle_status(&fs::read(dir.join("gpu_metrics")).ok()?)
}

/// One AMD GPU and the sensor files that answered when it was found.
#[derive(Debug)]
struct Card {
    device: String,
    dir: PathBuf,
    pci_slot: Option<String>,
    sensors: Vec<(SensorDescriptor, PathBuf, f64)>,
    /// Its `gpu_metrics` table reports the throttle status.
    throttle: bool,
}

impl Card {
    fn discover(device: String, dir: PathBuf) -> Self {
        let source = Source::new(AmdProvider::ID, "sysfs");
        let mut sensors = Vec::new();
        let mut add = |suffix: &str,
                       kind: SensorKind,
                       unit: Unit,
                       label: String,
                       path: PathBuf,
                       divisor: f64| {
            if read_f64(&path).is_none() {
                return;
            }
            let descriptor = SensorDescriptor {
                id: SensorId::new(format!("{device}/{suffix}")),
                kind,
                unit,
                device: device.clone(),
                label,
                source: source.clone(),
                limits: Default::default(),
            };
            sensors.push((descriptor, path, divisor));
        };

        let hwmon = fs::read_dir(dir.join("hwmon"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .min();
        if let Some(h) = &hwmon {
            for n in 1..=3 {
                let name = read(h.join(format!("temp{n}_label")))
                    .map_or_else(|| format!("Temperature {n}"), |l| label(&l));
                add(
                    &format!("temp{n}"),
                    SensorKind::Temperature,
                    Unit::Celsius,
                    name,
                    h.join(format!("temp{n}_input")),
                    1e3,
                );
            }
            let power = ["power1_input", "power1_average"]
                .map(|f| h.join(f))
                .into_iter()
                .find(|p| read_f64(p).is_some());
            if let Some(p) = power {
                add(
                    "power",
                    SensorKind::Power,
                    Unit::Watt,
                    "Board power".to_owned(),
                    p,
                    1e6,
                );
            }
            add(
                "power_limit",
                SensorKind::Power,
                Unit::Watt,
                "Power limit".to_owned(),
                h.join("power1_cap"),
                1e6,
            );
            for n in 1..=2 {
                let name = read(h.join(format!("freq{n}_label")))
                    .map_or_else(|| format!("Clock {n}"), |l| label(&l));
                add(
                    &format!("clock{n}"),
                    SensorKind::Clock,
                    Unit::Megahertz,
                    name,
                    h.join(format!("freq{n}_input")),
                    1e6,
                );
            }
            add(
                "fan",
                SensorKind::Fan,
                Unit::Rpm,
                "Fan speed".to_owned(),
                h.join("fan1_input"),
                1.0,
            );
            let name =
                read(h.join("in0_label")).map_or_else(|| "GPU voltage".to_owned(), |l| label(&l));
            add(
                "voltage",
                SensorKind::Voltage,
                Unit::Volt,
                name,
                h.join("in0_input"),
                1e3,
            );
        }
        add(
            "load",
            SensorKind::Load,
            Unit::Percent,
            "GPU load".to_owned(),
            dir.join("gpu_busy_percent"),
            1.0,
        );
        add(
            "memory_load",
            SensorKind::Load,
            Unit::Percent,
            "Memory controller load".to_owned(),
            dir.join("mem_busy_percent"),
            1.0,
        );
        add(
            "memory/used",
            SensorKind::Memory,
            Unit::Byte,
            "VRAM used".to_owned(),
            dir.join("mem_info_vram_used"),
            1.0,
        );

        let pci_slot = read(dir.join("uevent")).and_then(|u| {
            u.lines()
                .find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(str::to_owned))
        });
        let throttle = throttle_bits(&dir).is_some();
        Self {
            device,
            dir,
            pci_slot,
            sensors,
            throttle,
        }
    }
}

/// The AMD provider: every `amdgpu` card under `/sys/class/drm`.
#[derive(Debug)]
pub struct AmdProvider {
    cards: Vec<Card>,
    proc_root: PathBuf,
    sys_root: PathBuf,
    pci_ids: Vec<PathBuf>,
}

impl AmdProvider {
    /// Provider id used in every [`Source`] it reports.
    pub const ID: &'static str = "amd";

    /// Finds AMD GPUs on this machine (Linux only).
    pub fn new() -> Result<Self, ProviderError> {
        if !cfg!(target_os = "linux") {
            return Err(ProviderError::Unavailable {
                reason: "the amdgpu backend needs Linux".to_owned(),
            });
        }
        Self::with_roots(
            Path::new("/sys"),
            Path::new("/proc"),
            PCI_IDS.iter().map(PathBuf::from).collect(),
        )
    }

    /// Finds AMD GPUs under the given sysfs and procfs roots, naming them
    /// from the first readable `pci.ids` file. Used by tests with fixtures.
    pub fn with_roots(
        sys_root: &Path,
        proc_root: &Path,
        pci_ids: Vec<PathBuf>,
    ) -> Result<Self, ProviderError> {
        let mut cards: Vec<(u32, PathBuf)> = fs::read_dir(sys_root.join("class/drm"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let n: u32 = name.strip_prefix("card")?.parse().ok()?;
                Some((n, e.path().join("device")))
            })
            .filter(|(_, dir)| {
                read(dir.join("vendor")).as_deref() == Some(AMD_VENDOR)
                    && dir.join("mem_info_vram_total").exists()
            })
            .collect();
        cards.sort();
        if cards.is_empty() {
            return Err(ProviderError::Unavailable {
                reason: "no AMD GPU with the amdgpu driver found".to_owned(),
            });
        }
        let cards = cards
            .into_iter()
            .enumerate()
            .map(|(i, (_, dir))| Card::discover(format!("amd{i}"), dir))
            .collect();
        Ok(Self {
            cards,
            proc_root: proc_root.to_owned(),
            sys_root: sys_root.to_owned(),
            pci_ids,
        })
    }

    fn name(&self, card: &Card) -> Option<(String, &'static str)> {
        if let Some(name) = read(card.dir.join("product_name")) {
            return Some((name, "sysfs"));
        }
        let device = read(card.dir.join("device"))?;
        let db = self.pci_ids.iter().find_map(read)?;
        pci_name(&db, &device).map(|n| (n, "pci.ids"))
    }
}

impl Provider for AmdProvider {
    fn id(&self) -> &str {
        Self::ID
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::default();
        caps.set(Feature::GpuIdentity, Support::Available);
        caps.set(Feature::GpuSensors, Support::Available);
        caps.set(Feature::GpuProcesses, Support::Available);
        if self.cards.iter().any(|c| c.throttle) {
            caps.set(Feature::GpuThrottleReasons, Support::Available);
        } else {
            caps.set(
                Feature::GpuThrottleReasons,
                Support::Unsupported {
                    reason: "this GPU's gpu_metrics table has no ASIC-independent throttle status"
                        .to_owned(),
                },
            );
        }
        caps
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        let sysfs = Source::new(Self::ID, "sysfs");
        // A driver built as a module has a version; the one built into the
        // kernel goes by the kernel release.
        let driver = read(self.sys_root.join("module/amdgpu/version")).or_else(|| {
            read(self.proc_root.join("sys/kernel/osrelease"))
                .map(|k| format!("in kernel {}", k.trim()))
        });
        let gpus = self
            .cards
            .iter()
            .map(|card| GpuInfo {
                device: card.device.clone(),
                vendor: sysfs.tag(GpuVendor::Amd),
                name: self
                    .name(card)
                    .and_then(|(n, method)| Source::new(Self::ID, method).tag(n)),
                vram_bytes: read(card.dir.join("mem_info_vram_total"))
                    .and_then(|v| v.parse().ok())
                    .and_then(|v| sysfs.tag(v)),
                driver_version: driver.clone().and_then(|d| sysfs.tag(d)),
            })
            .collect();
        Ok(SystemInfo {
            gpus,
            ..SystemInfo::default()
        })
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        Ok(self
            .cards
            .iter()
            .flat_map(|c| c.sensors.iter().map(|(d, _, _)| d.clone()))
            .collect())
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        let at = Timestamp::now();
        Ok(self
            .cards
            .iter()
            .flat_map(|c| &c.sensors)
            .filter_map(|(d, path, divisor)| {
                Some(SensorReading {
                    id: d.id.clone(),
                    value: read_f64(path)? / divisor,
                    at,
                })
            })
            .collect())
    }

    fn gpu_processes(&self) -> Result<Vec<GpuProcess>, ProviderError> {
        let source = Source::new(Self::ID, "fdinfo");
        // (pid, card index) -> VRAM per DRM client, so fds sharing a client
        // are counted once.
        let mut usage: BTreeMap<(u32, usize), BTreeMap<Option<u64>, Option<u64>>> = BTreeMap::new();
        for entry in fs::read_dir(&self.proc_root)
            .into_iter()
            .flatten()
            .flatten()
        {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let fds = fs::read_dir(entry.path().join("fdinfo"));
            for fd in fds.into_iter().flatten().flatten() {
                let Some(info) = read(fd.path()).and_then(|t| parse_fdinfo(&t)) else {
                    continue;
                };
                if info.driver != "amdgpu" {
                    continue;
                }
                let Some(card) = self
                    .cards
                    .iter()
                    .position(|c| c.pci_slot.as_deref() == Some(info.pdev.as_str()))
                else {
                    continue;
                };
                usage
                    .entry((pid, card))
                    .or_default()
                    .insert(info.client_id, info.vram_bytes);
            }
        }
        Ok(usage
            .into_iter()
            .map(|((pid, card), clients)| {
                let vram: Option<u64> = clients.values().copied().sum();
                GpuProcess {
                    device: self.cards[card].device.clone(),
                    pid,
                    name: None,
                    kind: GpuProcessKind::Unknown,
                    used_memory_bytes: vram.and_then(|v| source.tag(v)),
                }
            })
            .collect())
    }

    fn throttle_reasons(&self) -> Result<Vec<ThrottleState>, ProviderError> {
        let source = Source::new(Self::ID, "gpu_metrics");
        Ok(self
            .cards
            .iter()
            .filter(|c| c.throttle)
            .filter_map(|c| {
                Some(ThrottleState {
                    device: c.device.clone(),
                    reasons: decode_throttle(throttle_bits(&c.dir)?),
                    source: source.clone(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCI_IDS_SAMPLE: &str = "\
# comment
1000  Broadcom / LSI
\t0001  Not this one
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]
\t\t1002 0e3b  Some subsystem
10de  NVIDIA Corporation
\t744c  Not AMD
";

    #[test]
    fn pci_ids_lookup() {
        assert_eq!(
            pci_name(PCI_IDS_SAMPLE, "0x744c").as_deref(),
            Some("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]")
        );
        assert_eq!(pci_name(PCI_IDS_SAMPLE, "0x0001"), None);
    }

    #[test]
    fn fdinfo_parsing() {
        let text = "pos:\t0\nflags:\t02100002\ndrm-driver:\tamdgpu\ndrm-pdev:\t0000:03:00.0\ndrm-client-id:\t42\ndrm-memory-vram:\t2048 KiB\n";
        assert_eq!(
            parse_fdinfo(text),
            Some(FdInfo {
                driver: "amdgpu".into(),
                pdev: "0000:03:00.0".into(),
                client_id: Some(42),
                vram_bytes: Some(2 * 1024 * 1024),
            })
        );
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t0\n"), None);
    }

    /// `struct gpu_metrics_v1_3` from `kgd_pp_interface.h`, field by field.
    #[repr(C)]
    #[allow(dead_code)]
    struct GpuMetricsV1_3 {
        structure_size: u16,
        format_revision: u8,
        content_revision: u8,
        temperatures: [u16; 6],
        activities: [u16; 3],
        average_socket_power: u16,
        energy_accumulator: u64,
        system_clock_counter: u64,
        average_clocks: [u16; 7],
        current_clocks: [u16; 7],
        throttle_status: u32,
        current_fan_speed: u16,
        pcie_link_width: u16,
        pcie_link_speed: u16,
        padding: u16,
        gfx_activity_acc: u32,
        mem_activity_acc: u32,
        temperature_hbm: [u16; 4],
        firmware_timestamp: u64,
        voltages: [u16; 3],
        padding1: u16,
        indep_throttle_status: u64,
    }

    /// `struct gpu_metrics_v2_2`; v2.3 and v2.4 only append fields to it.
    #[repr(C)]
    #[allow(dead_code)]
    struct GpuMetricsV2_2 {
        structure_size: u16,
        format_revision: u8,
        content_revision: u8,
        temperature_gfx: u16,
        temperature_soc: u16,
        temperature_core: [u16; 8],
        temperature_l3: [u16; 2],
        average_gfx_activity: u16,
        average_mm_activity: u16,
        system_clock_counter: u64,
        powers: [u16; 4],
        average_core_power: [u16; 8],
        average_clocks: [u16; 6],
        current_clocks: [u16; 6],
        current_coreclk: [u16; 8],
        current_l3clk: [u16; 2],
        throttle_status: u32,
        fan_pwm: u16,
        padding: [u16; 3],
        indep_throttle_status: u64,
    }

    #[test]
    fn metrics_offsets_match_the_kernel_structs() {
        assert_eq!(
            std::mem::offset_of!(GpuMetricsV1_3, indep_throttle_status),
            112
        );
        assert_eq!(std::mem::size_of::<GpuMetricsV1_3>(), 120);
        assert_eq!(
            std::mem::offset_of!(GpuMetricsV2_2, indep_throttle_status),
            120
        );
        assert_eq!(std::mem::size_of::<GpuMetricsV2_2>(), 128);
    }

    /// A `gpu_metrics` table as the kernel builds it: all `0xFF`, then the
    /// header, then the fields the ASIC fills.
    fn metrics_table(frev: u8, crev: u8, size: u16, indep: Option<(usize, u64)>) -> Vec<u8> {
        let mut t = vec![0xFF; usize::from(size)];
        t[..2].copy_from_slice(&size.to_ne_bytes());
        t[2] = frev;
        t[3] = crev;
        if let Some((offset, bits)) = indep {
            t[offset..offset + 8].copy_from_slice(&bits.to_ne_bytes());
        }
        t
    }

    #[test]
    fn indep_throttle_status_by_table_version() {
        let bits = 1 << 0 | 1 << 36;
        assert_eq!(
            indep_throttle_status(&metrics_table(1, 3, 120, Some((112, bits)))),
            Some(bits)
        );
        for crev in 2..=4 {
            let size = [128, 152, 168][usize::from(crev - 2)];
            assert_eq!(
                indep_throttle_status(&metrics_table(2, crev, size, Some((120, bits)))),
                Some(bits)
            );
        }
        // Running unrestricted.
        assert_eq!(
            indep_throttle_status(&metrics_table(1, 3, 120, Some((112, 0)))),
            Some(0)
        );
        // An ASIC that leaves the field as initialized (cyan_skillfish, v2.2).
        assert_eq!(indep_throttle_status(&metrics_table(2, 2, 128, None)), None);
        // Tables without the field: v1.0-1.2, v1.4 and later, v2.0-2.1, v3.0.
        for (frev, crev) in [(1, 2), (1, 4), (1, 8), (2, 1), (3, 0)] {
            assert_eq!(
                indep_throttle_status(&metrics_table(frev, crev, 128, Some((112, 0)))),
                None
            );
        }
        // Truncated tables.
        assert_eq!(indep_throttle_status(&metrics_table(1, 3, 112, None)), None);
        let mut short = metrics_table(1, 3, 120, Some((112, 0)));
        short.truncate(116);
        assert_eq!(indep_throttle_status(&short), None);
        assert_eq!(indep_throttle_status(&[]), None);
    }

    #[test]
    fn throttle_bits_decode() {
        assert!(decode_throttle(0).is_empty());
        // PPT0 and SPPT_APU are both the power limit, listed once.
        assert_eq!(decode_throttle(1 << 0 | 1 << 7), [ThrottleReason::PowerCap]);
        assert_eq!(
            decode_throttle(1 << 16 | 1 << 22),
            [ThrottleReason::CurrentLimit]
        );
        assert_eq!(decode_throttle(1 << 36), [ThrottleReason::SoftwareThermal]);
        assert_eq!(
            decode_throttle(1 << 44 | 1 << 47),
            [ThrottleReason::HardwareThermal]
        );
        // APCC, PPM, FIT and unknown bits are limits, just unclassified.
        for bit in [23, 56, 57, 63] {
            assert_eq!(decode_throttle(1 << bit), [ThrottleReason::Other]);
        }
        assert_eq!(
            decode_throttle(1 << 2 | 1 << 33 | 1 << 18),
            [
                ThrottleReason::PowerCap,
                ThrottleReason::CurrentLimit,
                ThrottleReason::SoftwareThermal
            ]
        );
    }

    /// A fake /sys + /proc tree under the temp dir.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(test: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("surtr-amd-{test}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn file(&self, path: &str, content: impl AsRef<[u8]>) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(test: &str) -> Fixture {
        let fx = Fixture::new(test);
        let dev = "sys/class/drm/card1/device";
        fx.file(&format!("{dev}/vendor"), "0x1002\n");
        fx.file(&format!("{dev}/device"), "0x744c\n");
        fx.file(
            &format!("{dev}/uevent"),
            "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
        );
        fx.file(&format!("{dev}/mem_info_vram_total"), "25753026560\n");
        fx.file(&format!("{dev}/mem_info_vram_used"), "1073741824\n");
        fx.file(&format!("{dev}/gpu_busy_percent"), "37\n");
        let hw = format!("{dev}/hwmon/hwmon3");
        fx.file(&format!("{hw}/temp1_input"), "45000\n");
        fx.file(&format!("{hw}/temp1_label"), "edge\n");
        fx.file(&format!("{hw}/temp2_input"), "61000\n");
        fx.file(&format!("{hw}/temp2_label"), "junction\n");
        fx.file(&format!("{hw}/power1_average"), "120000000\n");
        fx.file(&format!("{hw}/power1_cap"), "303000000\n");
        fx.file(&format!("{hw}/freq1_input"), "2500000000\n");
        fx.file(&format!("{hw}/freq1_label"), "sclk\n");
        fx.file(&format!("{hw}/fan1_input"), "1100\n");
        // An Intel iGPU and a connector entry that must be ignored.
        fx.file("sys/class/drm/card0/device/vendor", "0x8086\n");
        fx.file("sys/class/drm/card1-DP-1/status", "connected\n");
        fx.file("sys/module/amdgpu/version", "\n");
        // Process 100 holds one client through two fds; 200 is not amdgpu.
        let fdinfo = "drm-driver:\tamdgpu\ndrm-pdev:\t0000:03:00.0\ndrm-client-id:\t7\ndrm-memory-vram:\t512 MiB\n";
        fx.file("proc/100/fdinfo/5", fdinfo);
        fx.file("proc/100/fdinfo/6", fdinfo);
        fx.file("proc/100/fdinfo/0", "pos:\t0\n");
        fx.file(
            "proc/200/fdinfo/3",
            "drm-driver:\ti915\ndrm-pdev:\t0000:00:02.0\n",
        );
        fx.file("pci.ids", PCI_IDS_SAMPLE);
        fx
    }

    fn provider(fx: &Fixture) -> AmdProvider {
        AmdProvider::with_roots(
            &fx.0.join("sys"),
            &fx.0.join("proc"),
            vec![fx.0.join("pci.ids")],
        )
        .unwrap()
    }

    #[test]
    fn finds_card_and_names_it() {
        let fx = fixture("names");
        let p = provider(&fx);
        let info = p.system_info().unwrap();
        assert_eq!(info.gpus.len(), 1);
        let gpu = &info.gpus[0];
        assert_eq!(gpu.vendor.as_ref().unwrap().value, GpuVendor::Amd);
        let name = gpu.name.as_ref().unwrap();
        assert!(name.value.starts_with("Navi 31"));
        assert_eq!(name.source.method, "pci.ids");
        assert_eq!(gpu.vram_bytes.as_ref().unwrap().value, 25_753_026_560);
        // An empty version file means unknown, not "".
        assert!(gpu.driver_version.is_none());

        // The driver built into the kernel goes by the kernel release.
        fx.file("proc/sys/kernel/osrelease", "7.0.0-38-generic\n");
        let info = provider(&fx).system_info().unwrap();
        let driver = info.gpus[0].driver_version.as_ref().unwrap();
        assert_eq!(driver.value, "in kernel 7.0.0-38-generic");
    }

    #[test]
    fn sensors_scale_to_units() {
        let fx = fixture("sensors");
        let p = provider(&fx);
        let ids: Vec<String> = p.sensors().unwrap().into_iter().map(|d| d.id.0).collect();
        assert_eq!(
            ids,
            [
                "amd0/temp1",
                "amd0/temp2",
                "amd0/power",
                "amd0/power_limit",
                "amd0/clock1",
                "amd0/fan",
                "amd0/load",
                "amd0/memory/used",
            ]
        );
        let labels: Vec<String> = p.sensors().unwrap().into_iter().map(|d| d.label).collect();
        assert_eq!(labels[1], "Temperature (hotspot)");
        assert_eq!(labels[4], "Graphics clock");
        let values: Vec<f64> = p
            .read_sensors()
            .unwrap()
            .into_iter()
            .map(|r| r.value)
            .collect();
        assert_eq!(
            values,
            [
                45.0,
                61.0,
                120.0,
                303.0,
                2500.0,
                1100.0,
                37.0,
                1_073_741_824.0
            ]
        );
    }

    #[test]
    fn processes_from_fdinfo() {
        let fx = fixture("procs");
        let procs = provider(&fx).gpu_processes().unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].pid, 100);
        assert_eq!(procs[0].device, "amd0");
        assert_eq!(procs[0].kind, GpuProcessKind::Unknown);
        // Two fds of the same client count once.
        assert_eq!(
            procs[0].used_memory_bytes.as_ref().unwrap().value,
            512 << 20
        );
    }

    #[test]
    fn throttle_reasons_from_gpu_metrics() {
        let fx = fixture("throttle");
        let metrics = "sys/class/drm/card1/device/gpu_metrics";

        // No gpu_metrics file: unsupported, and no state rather than "none".
        let p = provider(&fx);
        assert!(matches!(
            p.capabilities().support(Feature::GpuThrottleReasons),
            Some(Support::Unsupported { .. })
        ));
        assert!(p.throttle_reasons().unwrap().is_empty());

        // A table the ASIC doesn't fill in: still unsupported.
        fx.file(metrics, metrics_table(2, 2, 128, None));
        let p = provider(&fx);
        assert!(matches!(
            p.capabilities().support(Feature::GpuThrottleReasons),
            Some(Support::Unsupported { .. })
        ));
        assert!(p.throttle_reasons().unwrap().is_empty());

        // An RDNA dGPU at its power limit and hotspot limit.
        fx.file(
            metrics,
            metrics_table(1, 3, 120, Some((112, 1 << 0 | 1 << 36))),
        );
        let p = provider(&fx);
        assert_eq!(
            p.capabilities().support(Feature::GpuThrottleReasons),
            Some(&Support::Available)
        );
        let states = p.throttle_reasons().unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].device, "amd0");
        assert_eq!(states[0].source.method, "gpu_metrics");
        assert_eq!(
            states[0].reasons,
            [ThrottleReason::PowerCap, ThrottleReason::SoftwareThermal]
        );

        // Read on every call, not cached at discovery.
        fx.file(metrics, metrics_table(1, 3, 120, Some((112, 0))));
        assert!(p.throttle_reasons().unwrap()[0].reasons.is_empty());
    }

    #[test]
    fn no_amd_card_is_unavailable() {
        let fx = Fixture::new("none");
        fx.file("sys/class/drm/card0/device/vendor", "0x10de\n");
        let err = AmdProvider::with_roots(&fx.0.join("sys"), &fx.0.join("proc"), vec![]);
        assert!(err.is_err());
    }
}
