//! Which logical CPUs share a physical core, and where stress threads go.
//!
//! Threads fill one per physical core before any SMT sibling is used, so
//! `--threads 4` on a 4-core, 8-thread CPU loads four cores, not two cores
//! twice. On some machines logical CPUs 0 and 1 are siblings, on others 0
//! and 4; only the topology says which.

use serde::{Deserialize, Serialize};

/// A logical CPU this process may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalCpu {
    /// Its number, as the OS counts it.
    pub id: u32,
    /// Its physical core: an index in order of first appearance.
    pub core: u32,
    /// Windows' efficiency class on hybrid CPUs (higher is faster, for
    /// example P-cores over E-cores); `None` where unknown.
    pub efficiency: Option<u8>,
}

/// Groups logical CPUs by physical core. `cpus` are `(id, key)` pairs, where
/// cpus with the same key share a core.
fn group<K: PartialEq>(mut cpus: Vec<(u32, K, Option<u8>)>) -> Vec<LogicalCpu> {
    cpus.sort_by_key(|c| c.0);
    let mut keys: Vec<K> = Vec::new();
    cpus.into_iter()
        .map(|(id, key, efficiency)| {
            let core = match keys.iter().position(|k| *k == key) {
                Some(i) => i,
                None => {
                    keys.push(key);
                    keys.len() - 1
                }
            };
            LogicalCpu {
                id,
                core: u32::try_from(core).unwrap_or(u32::MAX),
                efficiency,
            }
        })
        .collect()
}

/// The logical CPUs this process may use, with their cores.
pub fn topology() -> Vec<LogicalCpu> {
    let allowed = crate::cpu::allowed_cpus();
    #[cfg(target_os = "linux")]
    {
        let read = |id: u32, file: &str| -> Option<i64> {
            std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{id}/topology/{file}"))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        group(
            allowed
                .iter()
                .map(|&id| {
                    let key = (
                        read(id, "physical_package_id").unwrap_or(0),
                        // Unknown core: a core of its own.
                        read(id, "core_id").unwrap_or(-1 - i64::from(id)),
                    );
                    (id, key, None)
                })
                .collect(),
        )
    }
    #[cfg(windows)]
    {
        let cores = windows::cores();
        group(
            allowed
                .iter()
                .map(|&id| {
                    let found = cores
                        .iter()
                        .enumerate()
                        .find(|(_, (mask, _))| id < usize::BITS && mask >> id & 1 == 1);
                    match found {
                        Some((core, (_, eff))) => (id, core as i64, Some(*eff)),
                        None => (id, -1 - i64::from(id), None),
                    }
                })
                .collect(),
        )
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        group(allowed.iter().map(|&id| (id, id, None)).collect())
    }
}

#[cfg(windows)]
mod windows {
    #![allow(unsafe_code)]

    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };

    /// Each physical core in processor group 0: its logical-CPU mask and
    /// efficiency class.
    pub fn cores() -> Vec<(usize, u8)> {
        let mut len = 0u32;
        // SAFETY: a null buffer asks for the size.
        unsafe {
            GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                std::ptr::null_mut(),
                &raw mut len,
            )
        };
        if len == 0 {
            return Vec::new();
        }
        // u64 storage keeps the records aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds `len` writable bytes.
        let ok = unsafe {
            GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                buf.as_mut_ptr().cast(),
                &raw mut len,
            )
        };
        if ok == 0 {
            return Vec::new();
        }
        let bytes = buf.as_ptr().cast::<u8>();
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset < len as usize {
            // SAFETY: records are packed back to back within `len` bytes,
            // each starting with its own size.
            let info = unsafe {
                &*bytes
                    .add(offset)
                    .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()
            };
            if info.Size == 0 {
                break;
            }
            if info.Relationship == RelationProcessorCore {
                // SAFETY: a processor-core record holds the Processor variant.
                let p = unsafe { info.Anonymous.Processor };
                let mask = p.GroupMask[0];
                if mask.Group == 0 {
                    out.push((mask.Mask, p.EfficiencyClass));
                }
            }
            offset += info.Size as usize;
        }
        out
    }
}

/// The logical CPUs for `threads` threads (all, when `None`), one per
/// physical core first, then SMT siblings; with `smt` false, only one per
/// core.
pub fn placement(topo: &[LogicalCpu], threads: Option<u32>, smt: bool) -> Result<Vec<u32>, String> {
    let cores = topo.iter().map(|c| c.core).max().map_or(0, |m| m + 1);
    // Round robin over the cores: each core's first sibling, then the second.
    let mut order = Vec::new();
    let mut rank = 0;
    loop {
        let before = order.len();
        for core in 0..cores {
            if let Some(c) = topo.iter().filter(|c| c.core == core).nth(rank) {
                order.push(c.id);
            }
        }
        if order.len() == before || !smt {
            break;
        }
        rank += 1;
    }
    let n = threads.map_or(order.len(), |t| t as usize);
    if n == 0 {
        return Err("--threads must be at least 1".to_owned());
    }
    if n > order.len() {
        return Err(format!(
            "{n} threads, but only {} logical CPUs{} are available",
            order.len(),
            if smt { "" } else { " without SMT siblings" }
        ));
    }
    order.truncate(n);
    Ok(order)
}

/// Checks a hand-picked CPU list against the topology.
pub fn check(topo: &[LogicalCpu], cpus: &[u32]) -> Result<(), String> {
    if cpus.is_empty() {
        return Err("--cpus needs at least one CPU".to_owned());
    }
    for (i, c) in cpus.iter().enumerate() {
        if !topo.iter().any(|t| t.id == *c) {
            let all: Vec<String> = topo.iter().map(|t| t.id.to_string()).collect();
            return Err(format!(
                "CPU {c} is not available here (available: {})",
                all.join(",")
            ));
        }
        if cpus[..i].contains(c) {
            return Err(format!("CPU {c} is listed twice"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 cores, 8 threads, siblings numbered 0-3 then 4-7 (common on Intel
    /// and on Linux for many AMD parts).
    fn split() -> Vec<LogicalCpu> {
        group((0..8).map(|id| (id, id % 4, None)).collect())
    }

    /// 4 cores, 8 threads, siblings next to each other (0,1), (2,3), ...
    fn paired() -> Vec<LogicalCpu> {
        group((0..8).map(|id| (id, id / 2, None)).collect())
    }

    #[test]
    fn one_thread_per_core_first() {
        assert_eq!(placement(&split(), Some(4), true).unwrap(), [0, 1, 2, 3]);
        // With paired siblings, CPU 1 shares a core with CPU 0: skip it.
        assert_eq!(placement(&paired(), Some(4), true).unwrap(), [0, 2, 4, 6]);
        // Then the siblings.
        assert_eq!(
            placement(&paired(), None, true).unwrap(),
            [0, 2, 4, 6, 1, 3, 5, 7]
        );
        assert_eq!(
            placement(&paired(), Some(5), true).unwrap(),
            [0, 2, 4, 6, 1]
        );
    }

    #[test]
    fn without_smt_one_per_core() {
        assert_eq!(placement(&split(), None, false).unwrap(), [0, 1, 2, 3]);
        assert!(placement(&split(), Some(5), false).is_err());
        assert!(placement(&split(), Some(9), true).is_err());
        assert!(placement(&split(), Some(0), true).is_err());
    }

    #[test]
    fn checks_hand_picked_cpus() {
        assert!(check(&split(), &[0, 4]).is_ok());
        assert!(check(&split(), &[9]).unwrap_err().contains("not available"));
        assert!(check(&split(), &[1, 1]).unwrap_err().contains("twice"));
        assert!(check(&split(), &[]).is_err());
    }

    #[test]
    fn this_machine_has_a_topology() {
        let topo = topology();
        if cfg!(any(target_os = "linux", windows)) {
            assert!(!topo.is_empty());
            let all = placement(&topo, None, true).unwrap();
            assert_eq!(all.len(), topo.len(), "every CPU once");
        }
    }
}
