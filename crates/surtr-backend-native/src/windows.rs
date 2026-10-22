//! Windows: registry, `GlobalMemoryStatusEx`, `IsWow64Process2` and
//! `GetLogicalProcessorInformationEx`. No driver or elevation needed.

use std::{
    collections::HashMap,
    mem, ptr, slice,
    sync::{Mutex, PoisonError},
};

use surtr_model::{
    Arch, Capabilities, CpuInfo, Feature, MemoryInfo, OsInfo, SensorDescriptor, SensorId,
    SensorKind, SensorReading, Source, Support, SystemInfo, Timestamp,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_SUCCESS},
    System::{
        Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
            PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
            PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
        },
        Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
        SystemInformation::{
            GetLogicalProcessorInformationEx, GetSystemFirmwareTable, GlobalMemoryStatusEx,
            IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
            IMAGE_FILE_MACHINE_I386, LOGICAL_PROCESSOR_RELATIONSHIP, MEMORYSTATUSEX,
            RelationProcessorCore, RelationProcessorPackage,
            SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
        Threading::{
            GetActiveProcessorCount, GetActiveProcessorGroupCount, GetCurrentProcess,
            IsWow64Process2, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    },
};

use crate::{
    descriptor,
    parse::{self, GroupMasks, PdhInstance},
    reading,
};

pub const ID: &str = "windows";
pub const SUPPORTED: bool = true;

const OS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A `REG_SZ` value under `HKEY_LOCAL_MACHINE`, trimmed; empty counts as absent.
#[allow(unsafe_code)]
fn reg_string(subkey: &str, value: &str) -> Option<String> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut size: u32 = 0;
    // SAFETY: both strings are NUL-terminated; a null data pointer asks only
    // for the size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` has at least `size` writable bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..len]).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// A `REG_DWORD` value under `HKEY_LOCAL_MACHINE`.
#[allow(unsafe_code)]
fn reg_u32(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data: u32 = 0;
    let mut size = mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated; `data` has `size` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// The machine's native architecture, even when Surtr runs emulated (x64 on
/// ARM64).
#[allow(unsafe_code)]
fn native_arch() -> Option<Arch> {
    let mut process: IMAGE_FILE_MACHINE = 0;
    let mut native: IMAGE_FILE_MACHINE = 0;
    // SAFETY: the pseudo-handle from `GetCurrentProcess` is always valid, and
    // both out-pointers refer to live locals.
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) };
    if ok == 0 {
        return None;
    }
    Some(match native {
        IMAGE_FILE_MACHINE_AMD64 => Arch::X86_64,
        IMAGE_FILE_MACHINE_ARM64 => Arch::Aarch64,
        IMAGE_FILE_MACHINE_I386 => Arch::Other("x86".to_owned()),
        other => Arch::Other(format!("machine 0x{other:04x}")),
    })
}

/// The executable file name of process `pid`. Needs only limited query
/// rights, so it works for most processes of the current user.
#[allow(unsafe_code)]
pub fn process_name(pid: u32) -> Option<String> {
    // SAFETY: plain call; a null handle means failure.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `handle` is open; `buf` holds `len` UTF-16 units.
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    // SAFETY: `handle` is open and closed exactly once.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let name = path.rsplit(['\\', '/']).next()?.to_owned();
    (!name.is_empty()).then_some(name)
}

/// The raw SMBIOS structure table, without the 8-byte `RawSMBIOSData` header.
/// Needs no elevation.
#[allow(unsafe_code)]
fn smbios_table() -> Option<Vec<u8>> {
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    const HEADER: usize = 8;
    // SAFETY: a null buffer with size 0 asks only for the size.
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, ptr::null_mut(), 0) };
    if size as usize <= HEADER {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` has `size` writable bytes.
    let written = unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr().cast(), size) };
    if written == 0 || written > size {
        return None;
    }
    buf.truncate(written as usize);
    // RawSMBIOSData: calling method, major, minor, DMI revision (1 byte
    // each), table length (u32), then the table.
    let length = u32::from_le_bytes(buf.get(4..8)?.try_into().ok()?) as usize;
    let end = (HEADER + length).min(buf.len());
    Some(buf[HEADER..end].to_vec())
}

/// `(total, available)` physical memory visible to the OS, in bytes.
#[allow(unsafe_code)]
fn memory_status() -> Option<(u64, u64)> {
    // SAFETY: MEMORYSTATUSEX is plain data; all-zero is a valid value.
    let mut status: MEMORYSTATUSEX = unsafe { mem::zeroed() };
    status.dwLength = mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a live, correctly sized struct with `dwLength` set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.ullTotalPhys > 0).then_some((status.ullTotalPhys, status.ullAvailPhys))
}

/// Group masks of every processor record of one relationship kind.
#[allow(unsafe_code)]
fn processor_records(relation: LOGICAL_PROCESSOR_RELATIONSHIP) -> Option<Vec<GroupMasks>> {
    let mut len: u32 = 0;
    // SAFETY: a null buffer with zero length asks only for the size.
    unsafe { GetLogicalProcessorInformationEx(relation, ptr::null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    let record_size = mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    // Records are often shorter than the full struct (it is sized for the
    // largest union member), so pad the buffer by one struct: a reference to
    // the last record then stays inside the allocation. u64 storage keeps
    // records 8-byte aligned.
    let mut buf = vec![0u64; (len as usize + record_size).div_ceil(8)];
    // SAFETY: `buf` provides at least `len` writable bytes.
    let ok =
        unsafe { GetLogicalProcessorInformationEx(relation, buf.as_mut_ptr().cast(), &mut len) };
    if ok == 0 {
        return None;
    }

    let len = len as usize;
    let base = buf.as_ptr().cast::<u8>();
    let align = mem::align_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    // Relationship (4 bytes) + Size (4 bytes).
    const HEADER: usize = 8;
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset + HEADER <= len && offset.is_multiple_of(align) {
        // SAFETY: the offset is aligned and the padded buffer holds a full
        // struct from here (see above); only fields inside the record's
        // `Size` are read below.
        let record = unsafe {
            &*base
                .add(offset)
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()
        };
        let size = record.Size as usize;
        if size < HEADER || offset + size > len {
            break;
        }
        if record.Relationship == relation {
            // SAFETY: for processor relationships the union holds a
            // PROCESSOR_RELATIONSHIP. Core records may report a GroupCount of
            // 0 on old Windows versions; they always have one entry. The mask
            // array must end inside this record, which is checked first.
            let masks = unsafe {
                let processor = &record.Anonymous.Processor;
                let count = usize::from(processor.GroupCount).max(1);
                let start = processor.GroupMask.as_ptr();
                let end = start.add(count).cast::<u8>();
                if end > base.add(offset + size) {
                    break;
                }
                slice::from_raw_parts(start, count)
            };
            records.push(masks.iter().map(|g| (g.Group, g.Mask as u64)).collect());
        }
        offset += record.Size as usize;
    }
    Some(records)
}

pub fn system_info() -> SystemInfo {
    SystemInfo {
        os: os_info(),
        cpus: cpus(),
        memory: MemoryInfo {
            total_bytes: memory_status()
                .and_then(|(total, _)| Source::new(ID, "GlobalMemoryStatusEx").tag(total)),
            modules: smbios_table()
                .map(|t| crate::memory_modules(&t, &Source::new(ID, "smbios")))
                .unwrap_or_default(),
        },
        gpus: Vec::new(),
    }
}

fn os_info() -> OsInfo {
    let registry = Source::new(ID, "registry");
    let build = reg_string(OS_KEY, "CurrentBuild");
    let build_number = build.as_deref().and_then(|b| b.parse().ok());
    let name =
        reg_string(OS_KEY, "ProductName").map(|p| parse::windows_product_name(&p, build_number));
    let version = match (reg_string(OS_KEY, "DisplayVersion"), build) {
        (display, Some(build)) => {
            let build = match reg_u32(OS_KEY, "UBR") {
                Some(ubr) => format!("{build}.{ubr}"),
                None => build,
            };
            Some(match display {
                Some(d) => format!("{d} (build {build})"),
                None => format!("build {build}"),
            })
        }
        (display, None) => display,
    };
    OsInfo {
        name: name.and_then(|n| registry.tag(n)),
        version: version.and_then(|v| registry.tag(v)),
        kernel: None,
        arch: native_arch().and_then(|a| Source::new(ID, "IsWow64Process2").tag(a)),
    }
}

/// The maximum clock from CPUID leaf 0x16 (Intel). Hypervisors, including
/// Windows' own virtualization-based security, often report zeros there; then
/// the max clock stays unknown rather than falling back to the base clock.
#[cfg(target_arch = "x86_64")]
fn cpuid_max_mhz() -> Option<f64> {
    use std::arch::x86_64::__cpuid;
    if __cpuid(0).eax < 0x16 {
        return None;
    }
    let mhz = __cpuid(0x16).ebx & 0xffff;
    (mhz > 0).then(|| f64::from(mhz))
}

#[cfg(not(target_arch = "x86_64"))]
fn cpuid_max_mhz() -> Option<f64> {
    None
}

fn cpus() -> Vec<CpuInfo> {
    let registry = Source::new(ID, "registry");
    let topology = Source::new(ID, "GetLogicalProcessorInformationEx");
    let vendor = reg_string(CPU_KEY, "VendorIdentifier").map(|v| parse::cpu_vendor(&v));
    let brand = reg_string(CPU_KEY, "ProcessorNameString");
    // "~MHz" is the rated speed Windows measured at boot.
    let base = reg_u32(CPU_KEY, "~MHz").filter(|&m| m > 0).map(f64::from);
    let max = cpuid_max_mhz();

    let entry = |cores: Option<u32>, logical: Option<u32>| CpuInfo {
        vendor: vendor.clone().and_then(|v| registry.tag(v)),
        brand: brand.clone().and_then(|b| registry.tag(b)),
        physical_cores: cores.and_then(|c| topology.tag(c)),
        logical_cores: logical.and_then(|l| topology.tag(l)),
        base_clock_mhz: base.and_then(|b| registry.tag(b)),
        max_clock_mhz: max.and_then(|m| Source::new(ID, "cpuid").tag(m)),
    };

    let cores = processor_records(RelationProcessorCore).unwrap_or_default();
    if cores.is_empty() {
        return vec![entry(None, None)];
    }
    let packages = processor_records(RelationProcessorPackage).unwrap_or_default();
    parse::group_topology(&packages, &cores)
        .into_iter()
        .map(|(cores, logical)| entry(Some(cores), Some(logical)))
        .collect()
}

/// `PDH_FMT_NOCAP100` from `pdh.h` (not exported by windows-sys): don't clamp
/// percentages at 100, since processor performance exceeds 100 when boosting.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

const LOAD_COUNTER: &str = r"\Processor Information(*)\% Processor Time";
const PERF_COUNTER: &str = r"\Processor Information(*)\% Processor Performance";
const FREQ_COUNTER: &str = r"\Processor Information(*)\Processor Frequency";

/// A PDH query with the processor counters Surtr reads.
struct Pdh {
    query: PDH_HQUERY,
    load: PDH_HCOUNTER,
    perf: PDH_HCOUNTER,
    freq: PDH_HCOUNTER,
}

// SAFETY: PDH query and counter handles are not tied to the creating thread,
// and `State` keeps the `Pdh` behind a mutex so calls never overlap.
#[allow(unsafe_code)]
unsafe impl Send for Pdh {}

impl std::fmt::Debug for Pdh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pdh")
    }
}

impl Pdh {
    #[allow(unsafe_code)]
    fn open() -> Option<Self> {
        // SAFETY: handles are plain values; zero is the "no handle" value.
        let mut query: PDH_HQUERY = unsafe { mem::zeroed() };
        // SAFETY: a null data source means live data; `query` is a valid
        // out-pointer.
        if unsafe { PdhOpenQueryW(ptr::null(), 0, &mut query) } != ERROR_SUCCESS {
            return None;
        }
        let add = |path: &str| {
            let path = wide(path);
            // SAFETY: as above.
            let mut counter: PDH_HCOUNTER = unsafe { mem::zeroed() };
            // SAFETY: `query` is open, `path` is NUL-terminated and `counter`
            // is a valid out-pointer.
            let status = unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) };
            (status == ERROR_SUCCESS).then_some(counter)
        };
        let (Some(load), Some(perf), Some(freq)) =
            (add(LOAD_COUNTER), add(PERF_COUNTER), add(FREQ_COUNTER))
        else {
            // SAFETY: `query` is open and not used afterwards.
            unsafe { PdhCloseQuery(query) };
            return None;
        };
        let pdh = Self {
            query,
            load,
            perf,
            freq,
        };
        // Rate counters need two samples; take the first one now.
        pdh.collect();
        Some(pdh)
    }

    #[allow(unsafe_code)]
    fn collect(&self) -> bool {
        // SAFETY: `query` is open for the lifetime of `self`.
        unsafe { PdhCollectQueryData(self.query) == ERROR_SUCCESS }
    }

    /// Valid values of one counter, by processor instance.
    #[allow(unsafe_code)]
    fn values(&self, counter: PDH_HCOUNTER, format: u32) -> HashMap<PdhInstance, f64> {
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: a null buffer with zero size asks only for the size.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, ptr::null_mut())
        };
        if status != PDH_MORE_DATA || size == 0 {
            return HashMap::new();
        }
        // u64 storage keeps the item array 8-byte aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buf` has at least `size` writable bytes.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                format,
                &mut size,
                &mut count,
                buf.as_mut_ptr().cast(),
            )
        };
        if status != ERROR_SUCCESS {
            return HashMap::new();
        }
        // SAFETY: on success the buffer starts with `count` items; their name
        // pointers point at NUL-terminated strings later in the same buffer.
        let items = unsafe {
            slice::from_raw_parts(
                buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                count as usize,
            )
        };
        items
            .iter()
            .filter(|item| {
                let status = item.FmtValue.CStatus;
                status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
            })
            .filter_map(|item| {
                // SAFETY: see above; the name lives in `buf`.
                let name = unsafe {
                    let start = item.szName;
                    let mut len = 0;
                    while *start.add(len) != 0 {
                        len += 1;
                    }
                    String::from_utf16_lossy(slice::from_raw_parts(start, len))
                };
                // SAFETY: PDH_FMT_DOUBLE was requested, so the double member
                // is the initialized one.
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                Some((parse::pdh_instance(&name)?, value))
            })
            .collect()
    }
}

impl Drop for Pdh {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `query` is open and dropped exactly once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// First flat logical-processor number of each processor group, plus the
/// total at the end.
#[allow(unsafe_code)]
fn group_offsets() -> Vec<u32> {
    // SAFETY: plain queries without pointers.
    let groups = unsafe { GetActiveProcessorGroupCount() };
    let mut offsets = Vec::with_capacity(usize::from(groups) + 1);
    let mut next = 0;
    for group in 0..groups {
        offsets.push(next);
        // SAFETY: as above.
        next += unsafe { GetActiveProcessorCount(group) };
    }
    offsets.push(next);
    offsets
}

/// Sensor state: the PDH query (if the counters exist) and the processor
/// group layout used to number threads.
#[derive(Debug)]
pub struct State {
    pdh: Option<Mutex<Pdh>>,
    offsets: Vec<u32>,
}

impl State {
    pub fn new() -> Self {
        Self {
            pdh: Pdh::open().map(Mutex::new),
            offsets: group_offsets(),
        }
    }

    fn threads(&self) -> u32 {
        self.offsets.last().copied().unwrap_or(0)
    }

    fn thread_number(&self, group: u16, index: u32) -> Option<u32> {
        let start = *self.offsets.get(usize::from(group))?;
        let end = *self.offsets.get(usize::from(group) + 1)?;
        (start + index < end).then_some(start + index)
    }

    pub fn capabilities(&self, caps: &mut Capabilities) {
        if self.pdh.is_some() {
            caps.set(Feature::CpuLoad, Support::Available);
            caps.set(Feature::CpuClock, Support::Available);
        }
        // Without a driver Windows exposes none of these (plan, section 4).
        for feature in [
            Feature::CpuTemperature,
            Feature::CpuPower,
            Feature::CpuVoltage,
            Feature::MemoryTimings,
            Feature::BoardSensors,
            Feature::FanControl,
            Feature::CpuTuning,
        ] {
            caps.set(feature, Support::NeedsPrivilegedBackend);
        }
    }

    pub fn sensors(&self) -> Vec<SensorDescriptor> {
        let pdh = Source::new(ID, "pdh");
        let mut out = Vec::new();
        if self.pdh.is_some() {
            out.push(descriptor(
                "cpu/load",
                SensorKind::Load,
                "cpu",
                "Total load",
                &pdh,
            ));
            out.push(descriptor(
                "cpu/clock",
                SensorKind::Clock,
                "cpu",
                "Average clock (estimate)",
                &pdh,
            ));
            for n in 0..self.threads() {
                out.push(descriptor(
                    &format!("cpu/thread{n}/load"),
                    SensorKind::Load,
                    "cpu",
                    &format!("Thread {n} load"),
                    &pdh,
                ));
            }
            for n in 0..self.threads() {
                out.push(descriptor(
                    &format!("cpu/thread{n}/clock"),
                    SensorKind::Clock,
                    "cpu",
                    &format!("Thread {n} clock (estimate)"),
                    &pdh,
                ));
            }
        }
        out.push(descriptor(
            "memory/used",
            SensorKind::Memory,
            "memory",
            "Used",
            &Source::new(ID, "GlobalMemoryStatusEx"),
        ));
        out
    }

    fn id(&self, instance: PdhInstance, what: &str) -> Option<SensorId> {
        match instance {
            PdhInstance::Total => Some(SensorId::new(format!("cpu/{what}"))),
            PdhInstance::Thread { group, index } => {
                let n = self.thread_number(group, index)?;
                Some(SensorId::new(format!("cpu/thread{n}/{what}")))
            }
            PdhInstance::GroupTotal => None,
        }
    }

    pub fn read(&self) -> Vec<SensorReading> {
        let at = Timestamp::now();
        let mut out = Vec::new();

        if let Some(pdh) = &self.pdh {
            let pdh = pdh.lock().unwrap_or_else(PoisonError::into_inner);
            if pdh.collect() {
                let load = pdh.values(pdh.load, PDH_FMT_DOUBLE);
                let perf = pdh.values(pdh.perf, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100);
                let freq = pdh.values(pdh.freq, PDH_FMT_DOUBLE);
                let mut loads: Vec<_> = load
                    .into_iter()
                    .filter_map(|(i, v)| Some((self.id(i, "load")?, v)))
                    .collect();
                // Effective clock estimate: nominal frequency times the
                // performance ratio Windows reports.
                let mut clocks: Vec<_> = perf
                    .into_iter()
                    .filter_map(|(i, p)| {
                        let mhz = freq.get(&i)? * p / 100.0;
                        Some((self.id(i, "clock")?, mhz))
                    })
                    .filter(|(_, mhz)| *mhz > 0.0)
                    .collect();
                loads.sort_by(|a, b| a.0.cmp(&b.0));
                clocks.sort_by(|a, b| a.0.cmp(&b.0));
                out.extend(
                    loads
                        .into_iter()
                        .chain(clocks)
                        .map(|(id, v)| reading(id, v, at)),
                );
            }
        }

        if let Some((total, available)) = memory_status() {
            out.push(reading(
                SensorId::new("memory/used"),
                total.saturating_sub(available) as f64,
                at,
            ));
        }
        out
    }
}
