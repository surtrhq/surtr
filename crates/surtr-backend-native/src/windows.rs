//! Windows: registry, `GlobalMemoryStatusEx`, `IsWow64Process2` and
//! `GetLogicalProcessorInformationEx`. No driver or elevation needed.

use std::{mem, ptr, slice};

use surtr_model::{Arch, CpuInfo, MemoryInfo, OsInfo, Source, SystemInfo};
use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    System::{
        Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
        SystemInformation::{
            GetLogicalProcessorInformationEx, GlobalMemoryStatusEx, IMAGE_FILE_MACHINE,
            IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
            LOGICAL_PROCESSOR_RELATIONSHIP, MEMORYSTATUSEX, RelationProcessorCore,
            RelationProcessorPackage, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
        Threading::{GetCurrentProcess, IsWow64Process2},
    },
};

use crate::parse::{self, GroupMasks};

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

/// Physical memory visible to the OS, in bytes.
#[allow(unsafe_code)]
fn total_memory() -> Option<u64> {
    // SAFETY: MEMORYSTATUSEX is plain data; all-zero is a valid value.
    let mut status: MEMORYSTATUSEX = unsafe { mem::zeroed() };
    status.dwLength = mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a live, correctly sized struct with `dwLength` set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.ullTotalPhys > 0).then_some(status.ullTotalPhys)
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
            total_bytes: total_memory()
                .and_then(|t| Source::new(ID, "GlobalMemoryStatusEx").tag(t)),
            modules: Vec::new(),
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

fn cpus() -> Vec<CpuInfo> {
    let registry = Source::new(ID, "registry");
    let topology = Source::new(ID, "GetLogicalProcessorInformationEx");
    let vendor = reg_string(CPU_KEY, "VendorIdentifier").map(|v| parse::cpu_vendor(&v));
    let brand = reg_string(CPU_KEY, "ProcessorNameString");
    // "~MHz" is the rated speed Windows measured at boot.
    let base = reg_u32(CPU_KEY, "~MHz").filter(|&m| m > 0).map(f64::from);

    let entry = |cores: Option<u32>, logical: Option<u32>| CpuInfo {
        vendor: vendor.clone().and_then(|v| registry.tag(v)),
        brand: brand.clone().and_then(|b| registry.tag(b)),
        physical_cores: cores.and_then(|c| topology.tag(c)),
        logical_cores: logical.and_then(|l| topology.tag(l)),
        base_clock_mhz: base.and_then(|b| registry.tag(b)),
        max_clock_mhz: None,
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
