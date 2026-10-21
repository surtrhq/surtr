//! macOS: `sysctl` values, no root needed.

use std::{ffi::CString, ptr};

use surtr_model::{Arch, CpuInfo, CpuVendor, MemoryInfo, OsInfo, Source, SystemInfo};

use crate::parse;

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
