//! Minimal NVML bindings, written by hand from NVIDIA's public API
//! documentation (see NOTICE), and loaded at run time.
//!
//! Only the calls Surtr uses are declared. Every wrapper returns `None` when
//! NVML reports an error or "not supported", so callers never see a fake value.

use std::ffi::{c_char, c_uint, c_ulonglong, c_void};

use libloading::Library;

type Ret = c_uint;
/// An NVML device handle.
pub type Device = *mut c_void;

const SUCCESS: Ret = 0;
const INSUFFICIENT_SIZE: Ret = 7;
/// NVML_VALUE_NOT_AVAILABLE for 64-bit values.
const VALUE_NOT_AVAILABLE: c_ulonglong = c_ulonglong::MAX;

/// `nvmlMemory_t`. Some fields exist only to match the C layout.
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct Memory {
    total: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

/// `nvmlUtilization_t`.
#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

/// `nvmlProcessInfo_t` as used by the `_v2` and `_v3` process queries. Some
/// fields exist only to match the C layout.
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct ProcessInfo {
    pid: c_uint,
    used_gpu_memory: c_ulonglong,
    gpu_instance_id: c_uint,
    compute_instance_id: c_uint,
}

/// `nvmlClockType_t`.
#[derive(Debug, Clone, Copy)]
pub enum Clock {
    Graphics = 0,
    Memory = 2,
}

type FnVoid = unsafe extern "C" fn() -> Ret;
type FnString = unsafe extern "C" fn(*mut c_char, c_uint) -> Ret;
type FnCount = unsafe extern "C" fn(*mut c_uint) -> Ret;
type FnHandle = unsafe extern "C" fn(c_uint, *mut Device) -> Ret;
type FnDevString = unsafe extern "C" fn(Device, *mut c_char, c_uint) -> Ret;
type FnDevUint = unsafe extern "C" fn(Device, *mut c_uint) -> Ret;
type FnDevArgUint = unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> Ret;
type FnDevU64 = unsafe extern "C" fn(Device, *mut c_ulonglong) -> Ret;
type FnMemory = unsafe extern "C" fn(Device, *mut Memory) -> Ret;
type FnUtil = unsafe extern "C" fn(Device, *mut Utilization) -> Ret;
type FnProcs = unsafe extern "C" fn(Device, *mut c_uint, *mut ProcessInfo) -> Ret;

/// A loaded and initialized NVML. Shuts NVML down when dropped.
pub struct Nvml {
    shutdown: FnVoid,
    driver_version: FnString,
    device_count: FnCount,
    handle_by_index: FnHandle,
    name: FnDevString,
    memory: FnMemory,
    temperature: FnDevArgUint,
    power_usage: FnDevUint,
    power_limit: FnDevUint,
    utilization: FnUtil,
    clock: FnDevArgUint,
    fan_speed: FnDevUint,
    throttle: Option<FnDevU64>,
    compute_procs: Option<FnProcs>,
    graphics_procs: Option<FnProcs>,
    // Keeps the library mapped while the function pointers above are in use;
    // declared last so it is dropped last.
    _library: Library,
}

// SAFETY: NVIDIA documents NVML as thread-safe; the struct holds only function
// pointers into the loaded library and the library handle.
#[allow(unsafe_code)]
unsafe impl Send for Nvml {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Nvml {}

#[allow(unsafe_code)]
fn open_library() -> Result<Library, String> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{LOAD_LIBRARY_SEARCH_SYSTEM32, Library as WinLibrary};
        // Only System32: never the current directory or PATH, so a planted
        // nvml.dll can't be picked up.
        // SAFETY: loading NVML runs its initializers, which is what the
        // driver install intends; nothing else is loaded.
        unsafe { WinLibrary::load_with_flags("nvml.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map(Library::from)
            .map_err(|e| format!("NVML not found (nvml.dll): {e}"))
    }
    #[cfg(not(windows))]
    {
        // SAFETY: as above, with the soname the driver installs.
        unsafe { Library::new("libnvidia-ml.so.1") }
            .map_err(|e| format!("NVML not found (libnvidia-ml.so.1): {e}"))
    }
}

impl Nvml {
    /// Loads and initializes NVML.
    #[allow(unsafe_code)]
    pub fn load() -> Result<Self, String> {
        let library = open_library()?;
        // SAFETY: each symbol is looked up with the signature NVIDIA documents
        // for it; the copied function pointers stay valid while `library`
        // lives, and `library` is stored in the returned struct.
        unsafe {
            fn get<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
                // SAFETY: see the caller.
                unsafe { library.get::<T>(name) }
                    .map(|s| *s)
                    .map_err(|e| format!("NVML is missing {}: {e}", String::from_utf8_lossy(name)))
            }
            fn optional<T: Copy>(library: &Library, names: &[&[u8]]) -> Option<T> {
                names
                    .iter()
                    // SAFETY: see the caller.
                    .find_map(|&name| unsafe { library.get::<T>(name) }.ok().map(|s| *s))
            }
            let init: FnVoid = get(&library, b"nvmlInit_v2\0")?;
            let status = init();
            if status != SUCCESS {
                return Err(format!("NVML failed to initialize (error {status})"));
            }
            Ok(Self {
                shutdown: get(&library, b"nvmlShutdown\0")?,
                driver_version: get(&library, b"nvmlSystemGetDriverVersion\0")?,
                device_count: get(&library, b"nvmlDeviceGetCount_v2\0")?,
                handle_by_index: get(&library, b"nvmlDeviceGetHandleByIndex_v2\0")?,
                name: get(&library, b"nvmlDeviceGetName\0")?,
                memory: get(&library, b"nvmlDeviceGetMemoryInfo\0")?,
                temperature: get(&library, b"nvmlDeviceGetTemperature\0")?,
                power_usage: get(&library, b"nvmlDeviceGetPowerUsage\0")?,
                power_limit: get(&library, b"nvmlDeviceGetEnforcedPowerLimit\0")?,
                utilization: get(&library, b"nvmlDeviceGetUtilizationRates\0")?,
                clock: get(&library, b"nvmlDeviceGetClockInfo\0")?,
                fan_speed: get(&library, b"nvmlDeviceGetFanSpeed\0")?,
                throttle: optional(
                    &library,
                    &[
                        b"nvmlDeviceGetCurrentClocksEventReasons\0",
                        b"nvmlDeviceGetCurrentClocksThrottleReasons\0",
                    ],
                ),
                compute_procs: optional(
                    &library,
                    &[
                        b"nvmlDeviceGetComputeRunningProcesses_v3\0",
                        b"nvmlDeviceGetComputeRunningProcesses_v2\0",
                    ],
                ),
                graphics_procs: optional(
                    &library,
                    &[
                        b"nvmlDeviceGetGraphicsRunningProcesses_v3\0",
                        b"nvmlDeviceGetGraphicsRunningProcesses_v2\0",
                    ],
                ),
                _library: library,
            })
        }
    }

    /// Whether per-process queries are available.
    pub fn has_processes(&self) -> bool {
        self.compute_procs.is_some() || self.graphics_procs.is_some()
    }

    #[allow(unsafe_code)]
    fn string(f: impl FnOnce(*mut c_char, c_uint) -> Ret) -> Option<String> {
        let mut buf = [0 as c_char; 128];
        if f(buf.as_mut_ptr(), buf.len() as c_uint) != SUCCESS {
            return None;
        }
        // SAFETY: on success NVML wrote a NUL-terminated string into `buf`
        // (it fails with INSUFFICIENT_SIZE rather than truncating).
        let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
        let s = s.to_string_lossy().trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// The driver version, for example "610.60".
    #[allow(unsafe_code)]
    pub fn driver_version(&self) -> Option<String> {
        // SAFETY: buffer pointer and length describe a live buffer.
        Self::string(|p, n| unsafe { (self.driver_version)(p, n) })
    }

    /// Number of NVIDIA GPUs.
    #[allow(unsafe_code)]
    pub fn device_count(&self) -> u32 {
        let mut count = 0;
        // SAFETY: `count` is a valid out-pointer.
        let ok = unsafe { (self.device_count)(&mut count) } == SUCCESS;
        if ok { count } else { 0 }
    }

    /// The handle of GPU `index`.
    #[allow(unsafe_code)]
    pub fn device(&self, index: u32) -> Option<Device> {
        let mut device: Device = std::ptr::null_mut();
        // SAFETY: `device` is a valid out-pointer.
        let ok = unsafe { (self.handle_by_index)(index, &mut device) } == SUCCESS;
        ok.then_some(device)
    }

    /// Product name.
    #[allow(unsafe_code)]
    pub fn name(&self, device: Device) -> Option<String> {
        // SAFETY: `device` came from NVML; buffer pointer and length describe a
        // live buffer.
        Self::string(|p, n| unsafe { (self.name)(device, p, n) })
    }

    /// `(total, used)` video memory in bytes.
    #[allow(unsafe_code)]
    pub fn memory(&self, device: Device) -> Option<(u64, u64)> {
        let mut m = Memory::default();
        // SAFETY: `device` came from NVML and `m` is a valid out-pointer.
        let ok = unsafe { (self.memory)(device, &mut m) } == SUCCESS;
        (ok && m.total > 0).then_some((m.total, m.used))
    }

    /// Core temperature in °C.
    #[allow(unsafe_code)]
    pub fn temperature(&self, device: Device) -> Option<u32> {
        let mut t = 0;
        // SAFETY: as above; sensor 0 is NVML_TEMPERATURE_GPU.
        let ok = unsafe { (self.temperature)(device, 0, &mut t) } == SUCCESS;
        ok.then_some(t)
    }

    /// Board power draw in milliwatts.
    #[allow(unsafe_code)]
    pub fn power_mw(&self, device: Device) -> Option<u32> {
        let mut p = 0;
        // SAFETY: as above.
        let ok = unsafe { (self.power_usage)(device, &mut p) } == SUCCESS;
        ok.then_some(p)
    }

    /// Enforced power limit in milliwatts.
    #[allow(unsafe_code)]
    pub fn power_limit_mw(&self, device: Device) -> Option<u32> {
        let mut p = 0;
        // SAFETY: as above.
        let ok = unsafe { (self.power_limit)(device, &mut p) } == SUCCESS;
        (ok && p > 0).then_some(p)
    }

    /// `(gpu, memory controller)` utilization in percent.
    #[allow(unsafe_code)]
    pub fn utilization(&self, device: Device) -> Option<(u32, u32)> {
        let mut u = Utilization::default();
        // SAFETY: as above.
        let ok = unsafe { (self.utilization)(device, &mut u) } == SUCCESS;
        ok.then_some((u.gpu, u.memory))
    }

    /// Current clock in MHz.
    #[allow(unsafe_code)]
    pub fn clock(&self, device: Device, clock: Clock) -> Option<u32> {
        let mut mhz = 0;
        // SAFETY: as above.
        let ok = unsafe { (self.clock)(device, clock as c_uint, &mut mhz) } == SUCCESS;
        (ok && mhz > 0).then_some(mhz)
    }

    /// Fan speed as a percentage of its maximum.
    #[allow(unsafe_code)]
    pub fn fan_percent(&self, device: Device) -> Option<u32> {
        let mut f = 0;
        // SAFETY: as above.
        let ok = unsafe { (self.fan_speed)(device, &mut f) } == SUCCESS;
        ok.then_some(f)
    }

    /// The raw clock event (throttle) reason bit mask.
    #[allow(unsafe_code)]
    pub fn throttle_bits(&self, device: Device) -> Option<u64> {
        let f = self.throttle?;
        let mut bits = 0;
        // SAFETY: as above.
        let ok = unsafe { f(device, &mut bits) } == SUCCESS;
        ok.then_some(bits)
    }

    /// `(pid, used bytes)` of compute or graphics processes; used bytes are
    /// `None` when the driver doesn't report them.
    #[allow(unsafe_code)]
    pub fn processes(&self, device: Device, compute: bool) -> Option<Vec<(u32, Option<u64>)>> {
        let f = if compute {
            self.compute_procs
        } else {
            self.graphics_procs
        }?;
        // Processes can start between the two calls, so retry a few times.
        let mut count: c_uint = 0;
        // SAFETY: a zero count with a null buffer asks for the number needed.
        let status = unsafe { f(device, &mut count, std::ptr::null_mut()) };
        match status {
            SUCCESS => return Some(Vec::new()),
            INSUFFICIENT_SIZE => {}
            _ => return None,
        }
        for _ in 0..3 {
            let mut infos = vec![ProcessInfo::default(); count as usize + 8];
            let mut n = infos.len() as c_uint;
            // SAFETY: `infos` has room for `n` entries.
            let status = unsafe { f(device, &mut n, infos.as_mut_ptr()) };
            match status {
                SUCCESS => {
                    infos.truncate(n as usize);
                    return Some(
                        infos
                            .iter()
                            .map(|p| {
                                let used = (p.used_gpu_memory != VALUE_NOT_AVAILABLE)
                                    .then_some(p.used_gpu_memory);
                                (p.pid, used)
                            })
                            .collect(),
                    );
                }
                INSUFFICIENT_SIZE => count = n,
                _ => return None,
            }
        }
        None
    }
}

impl Drop for Nvml {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: NVML was initialized in `load` and is shut down exactly once.
        unsafe { (self.shutdown)() };
    }
}
