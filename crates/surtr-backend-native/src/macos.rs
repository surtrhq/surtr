//! macOS: `sysctl` values and mach host statistics, no root needed.

use std::{
    ffi::CString,
    mem, ptr, slice,
    sync::{Mutex, PoisonError},
};

use surtr_model::{
    Arch, Capabilities, CpuInfo, CpuVendor, Feature, MemoryInfo, OsInfo, SensorDescriptor,
    SensorId, SensorKind, SensorReading, Source, Support, SystemInfo, Timestamp,
};

use crate::{
    descriptor,
    parse::{self, CpuTimes},
    reading,
};

pub const ID: &str = "macos";
pub const SUPPORTED: bool = true;

/// Raw bytes of a sysctl value.
#[allow(unsafe_code)]
fn sysctl_bytes(name: &str) -> Option<Vec<u8>> {
    let name = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: `name` is NUL-terminated; a null buffer asks only for the size.
    let rc =
        unsafe { libc::sysctlbyname(name.as_ptr(), ptr::null_mut(), &mut len, ptr::null_mut(), 0) };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` has `len` writable bytes and `len` says so.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

fn sysctl_string(name: &str) -> Option<String> {
    let bytes = sysctl_bytes(name)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// An integer sysctl, which may be 32 or 64 bits wide.
fn sysctl_u64(name: &str) -> Option<u64> {
    let bytes = sysctl_bytes(name)?;
    match bytes.len() {
        4 => Some(u64::from(u32::from_ne_bytes(bytes.try_into().ok()?))),
        8 => Some(u64::from_ne_bytes(bytes.try_into().ok()?)),
        _ => None,
    }
}

fn sysctl_u32(name: &str) -> Option<u32> {
    sysctl_u64(name).and_then(|v| u32::try_from(v).ok())
}

pub fn system_info() -> SystemInfo {
    let src = Source::new(ID, "sysctl");
    let apple_silicon = sysctl_u64("hw.optional.arm64") == Some(1);

    let os = OsInfo {
        name: src.tag("macOS".to_owned()),
        version: sysctl_string("kern.osproductversion").and_then(|v| src.tag(v)),
        kernel: sysctl_string("kern.osrelease").and_then(|v| src.tag(v)),
        arch: src.tag(if apple_silicon {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }),
    };

    let vendor = if apple_silicon {
        Some(CpuVendor::Apple)
    } else {
        sysctl_string("machdep.cpu.vendor").map(|v| parse::cpu_vendor(&v))
    };
    // macOS reports totals; split per package only when there is one package.
    let single_package = sysctl_u32("hw.packages").is_none_or(|n| n <= 1);
    let count = |name: &str| sysctl_u32(name).filter(|_| single_package);
    let mhz = |name: &str| {
        sysctl_u64(name)
            .filter(|&hz| hz > 0)
            .map(|hz| hz as f64 / 1e6)
    };
    let cpu = CpuInfo {
        vendor: vendor.and_then(|v| src.tag(v)),
        brand: sysctl_string("machdep.cpu.brand_string").and_then(|b| src.tag(b)),
        physical_cores: count("hw.physicalcpu").and_then(|c| src.tag(c)),
        logical_cores: count("hw.logicalcpu").and_then(|c| src.tag(c)),
        base_clock_mhz: mhz("hw.cpufrequency").and_then(|m| src.tag(m)),
        max_clock_mhz: mhz("hw.cpufrequency_max").and_then(|m| src.tag(m)),
    };

    SystemInfo {
        os,
        cpus: vec![cpu],
        memory: MemoryInfo {
            total_bytes: sysctl_u64("hw.memsize").and_then(|m| src.tag(m)),
            modules: Vec::new(),
        },
        gpus: Vec::new(),
    }
}

/// Busy and total ticks per logical CPU from `host_processor_info`.
// mach_host_self and mach_task_self are deprecated in libc in favor of the
// mach2 crate, but they are the documented system calls.
#[allow(unsafe_code, deprecated)]
fn cpu_ticks() -> Option<Vec<CpuTimes>> {
    let mut cpus: libc::natural_t = 0;
    let mut info: libc::processor_info_array_t = ptr::null_mut();
    let mut len: libc::mach_msg_type_number_t = 0;
    // SAFETY: all out-pointers refer to locals; the kernel allocates `info`.
    let kr = unsafe {
        libc::host_processor_info(
            libc::mach_host_self(),
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut cpus,
            &mut info,
            &mut len,
        )
    };
    if kr != libc::KERN_SUCCESS || info.is_null() {
        return None;
    }
    // SAFETY: on success `info` holds `len` integers, 4 per CPU (user,
    // system, idle, nice).
    let ticks = unsafe { slice::from_raw_parts(info, len as usize) };
    let out = ticks
        .chunks_exact(4)
        .take(cpus as usize)
        .map(|t| {
            let tick = |i: usize| u64::from(t[i] as u32);
            let busy = tick(0) + tick(1) + tick(3);
            CpuTimes {
                busy,
                total: busy + tick(2),
            }
        })
        .collect();
    // SAFETY: `info` was allocated by the kernel for this task with `len`
    // integers and is freed exactly once.
    unsafe {
        libc::vm_deallocate(
            libc::mach_task_self(),
            info as libc::vm_address_t,
            len as libc::vm_size_t * mem::size_of::<libc::integer_t>() as libc::vm_size_t,
        )
    };
    Some(out)
}

/// Memory in use the way Activity Monitor counts it: app memory (internal
/// minus purgeable pages) plus wired plus compressed.
#[allow(unsafe_code, deprecated)]
fn memory_used() -> Option<u64> {
    // SAFETY: vm_statistics64 is plain data; all-zero is valid.
    let mut stats: libc::vm_statistics64 = unsafe { mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `stats` has room for `count` integers.
    let kr = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut stats).cast(),
            &mut count,
        )
    };
    if kr != libc::KERN_SUCCESS {
        return None;
    }
    let page = sysctl_u64("hw.pagesize")?;
    let (internal, purgeable, wired, compressed) = (
        stats.internal_page_count,
        stats.purgeable_count,
        stats.wire_count,
        stats.compressor_page_count,
    );
    let pages = u64::from(internal).saturating_sub(u64::from(purgeable))
        + u64::from(wired)
        + u64::from(compressed);
    Some(pages * page)
}

/// Sensor state: the previous tick sample per CPU, for loads.
#[derive(Debug)]
pub struct State {
    prev: Mutex<Vec<CpuTimes>>,
    threads: usize,
}

impl State {
    pub fn new() -> Self {
        let prev = cpu_ticks().unwrap_or_default();
        Self {
            threads: prev.len(),
            prev: Mutex::new(prev),
        }
    }

    pub fn capabilities(&self, caps: &mut Capabilities) {
        if self.threads > 0 {
            caps.set(Feature::CpuLoad, Support::Available);
        }
    }

    pub fn sensors(&self) -> Vec<SensorDescriptor> {
        let src = Source::new(ID, "host_processor_info");
        let mut out = Vec::new();
        if self.threads > 0 {
            out.push(descriptor(
                "cpu/load",
                SensorKind::Load,
                "cpu",
                "Total load",
                &src,
            ));
            for n in 0..self.threads {
                out.push(descriptor(
                    &format!("cpu/thread{n}/load"),
                    SensorKind::Load,
                    "cpu",
                    &format!("Thread {n} load"),
                    &src,
                ));
            }
        }
        out.push(descriptor(
            "memory/used",
            SensorKind::Memory,
            "memory",
            "Used",
            &Source::new(ID, "host_statistics64"),
        ));
        out
    }

    pub fn read(&self) -> Vec<SensorReading> {
        let at = Timestamp::now();
        let mut out = Vec::new();
        if let Some(cur) = cpu_ticks() {
            let mut prev = self.prev.lock().unwrap_or_else(PoisonError::into_inner);
            let sum = |t: &[CpuTimes]| CpuTimes {
                busy: t.iter().map(|c| c.busy).sum(),
                total: t.iter().map(|c| c.total).sum(),
            };
            if prev.len() == cur.len() {
                if let Some(load) = parse::load_percent(sum(&prev), sum(&cur)) {
                    out.push(reading(SensorId::new("cpu/load"), load, at));
                }
                for (n, (p, c)) in prev.iter().zip(&cur).enumerate() {
                    if let Some(load) = parse::load_percent(*p, *c) {
                        out.push(reading(
                            SensorId::new(format!("cpu/thread{n}/load")),
                            load,
                            at,
                        ));
                    }
                }
            }
            *prev = cur;
        }
        if let Some(used) = memory_used() {
            out.push(reading(SensorId::new("memory/used"), used as f64, at));
        }
        out
    }
}

#[allow(unsafe_code)]
pub fn process_name(pid: u32) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for the size passed.
    let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let len = usize::try_from(len).ok().filter(|&n| n > 0)?;
    Some(String::from_utf8_lossy(&buf[..len]).into_owned())
}

/// Not detected on macOS yet.
pub fn display_adapters() -> Vec<crate::DisplayAdapter> {
    Vec::new()
}
