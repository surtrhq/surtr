//! The built-in CPU test (plan, section 5.3), run inside a worker process.
//!
//! Every thread repeats known-answer kernels: deterministic integer and
//! floating-point computations whose results are worked out once before the
//! threads start. Rust does not reorder floating-point math, so a healthy CPU
//! gives bit-identical answers every time; any difference is a hardware
//! error (too little voltage, too much clock, too much heat).

use std::{
    hint::black_box,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    Isolation, Kernels, Params, Workload,
    event::{Event, SCHEMA},
    pattern::{Pattern, Schedule},
};

/// Seeds each kernel cycles through, so a thread never repeats one result.
const SEEDS: [u64; 4] = [
    0x243F_6A88_85A3_08D3,
    0x1319_8A2E_0370_7344,
    0xA409_3822_299F_31D0,
    0x082E_FA98_EC4E_6C89,
];

/// Errors a thread reports one by one before it only counts them.
const REPORTED_PER_THREAD: u32 = 10;

/// Integer work: xorshift, multiply and rotate, about 65 000 steps.
fn integer(seed: u64) -> u64 {
    let mut x = seed;
    let mut acc = 0u64;
    for i in 0..65_536u32 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        acc = acc
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(x.rotate_left(i & 63));
    }
    acc
}

/// Floating-point work: a 48x48 matrix product plus square roots, returned as
/// the bits of the sum.
fn float(seed: u64) -> u64 {
    const N: usize = 48;
    let mut a = [0.0f64; N * N];
    let mut b = [0.0f64; N * N];
    let mut x = seed;
    for v in a.iter_mut().chain(b.iter_mut()) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // Values in [0.5, 1.5).
        *v = 0.5 + (x >> 11) as f64 / (1u64 << 53) as f64;
    }
    let mut sum = 0.0f64;
    for i in 0..N {
        for j in 0..N {
            let mut dot = 0.0f64;
            for k in 0..N {
                dot += a[i * N + k] * b[k * N + j];
            }
            sum += dot.sqrt();
        }
    }
    sum.to_bits()
}

/// A kernel: its name and the computation.
type Kernel = (&'static str, fn(u64) -> u64);

const MIXED: [Kernel; 2] = [("integer", integer), ("float", float)];

/// The kernels for `set`, picking the best SIMD variant this CPU runs.
fn kernels(set: Kernels) -> Vec<Kernel> {
    match set {
        Kernels::Mixed => MIXED.to_vec(),
        Kernels::Simd => vec![crate::simd::best()],
    }
}

/// The logical CPUs this process may run on (a cgroup, `taskset` or job
/// object may allow fewer than the machine has), in order. Empty where the
/// OS doesn't say.
pub fn allowed_cpus() -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: cpu_set_t is plain data; the call only writes it.
        #[allow(unsafe_code)]
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            if libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &raw mut set) != 0 {
                return Vec::new();
            }
            (0..libc::CPU_SETSIZE as usize)
                .filter(|&c| libc::CPU_ISSET(c, &set))
                .filter_map(|c| u32::try_from(c).ok())
                .collect()
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: the pseudo-handle is always valid; both are out-pointers.
        #[allow(unsafe_code)]
        let ok = unsafe {
            GetProcessAffinityMask(GetCurrentProcess(), &raw mut process, &raw mut system)
        };
        if ok == 0 {
            return Vec::new();
        }
        (0..usize::BITS)
            .filter(|&c| process >> c & 1 == 1)
            .collect()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

/// Pins the calling thread to logical CPU `cpu`, where the OS allows it.
/// Returns whether it worked.
fn pin(cpu: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: cpu_set_t is plain data; the calls only read and write it.
        #[allow(unsafe_code)]
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu as usize, &mut set);
            libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set) == 0
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};
        if cpu >= usize::BITS {
            return false;
        }
        // SAFETY: the pseudo-handle of the current thread is always valid.
        #[allow(unsafe_code)]
        unsafe {
            SetThreadAffinityMask(GetCurrentThread(), 1usize << cpu) != 0
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = cpu;
        false
    }
}

/// The built-in CPU test as a [`Workload`].
#[derive(Debug, Clone, Copy)]
pub struct Cpu;

impl Workload for Cpu {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn description(&self) -> &'static str {
        "Loads every CPU thread with integer and floating-point computations whose \
         answers are known, and reports any wrong answer with its CPU."
    }

    fn isolation(&self) -> Isolation {
        Isolation::Required
    }

    fn run(&self, params: Params, out: &mut dyn Write) -> std::io::Result<()> {
        run(params, out)
    }
}

/// Runs the test and writes events to `out` (the worker's standard output)
/// until `params.duration` has passed. The host stops it earlier by killing
/// the process.
pub fn run(params: Params, out: &mut dyn Write) -> std::io::Result<()> {
    // The answers, worked out before any heavy load.
    let kernels = Arc::new(kernels(params.kernels));
    let expected: Vec<Vec<u64>> = kernels
        .iter()
        .map(|(_, k)| SEEDS.iter().map(|&s| k(s)).collect())
        .collect();
    let expected = Arc::new(expected);
    let threads = if params.cpus.is_empty() {
        params.threads.max(1)
    } else {
        u32::try_from(params.cpus.len()).unwrap_or(u32::MAX)
    };
    let counts: Arc<Vec<AtomicU64>> = Arc::new((0..threads).map(|_| AtomicU64::new(0)).collect());
    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let schedule =
        Arc::new((params.pattern == Pattern::Transient).then(|| Schedule::new(params.duration)));
    let (errors, inbox) = mpsc::channel::<Event>();
    // The host's placement, else thread t on the t-th CPU this process may
    // use.
    let allowed = Arc::new(if params.cpus.is_empty() {
        allowed_cpus()
    } else {
        params.cpus.clone()
    });

    writeln!(
        out,
        "{}",
        Event::Started {
            schema: SCHEMA,
            workload: "cpu".to_owned(),
            threads,
            kernels: kernels.iter().map(|(n, _)| (*n).to_owned()).collect(),
        }
        .to_line()
    )?;
    out.flush()?;

    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let (expected, counts, stop, errors, allowed, schedule, kernels) = (
                Arc::clone(&expected),
                Arc::clone(&counts),
                Arc::clone(&stop),
                errors.clone(),
                Arc::clone(&allowed),
                Arc::clone(&schedule),
                Arc::clone(&kernels),
            );
            thread::spawn(move || {
                let cpu = (!allowed.is_empty())
                    .then(|| allowed[t as usize % allowed.len()])
                    .filter(|&c| pin(c));
                let mut reported = 0;
                let mut round = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    // Transient: every thread rests through the same gaps.
                    if let Some(rest) = schedule
                        .as_ref()
                        .as_ref()
                        .and_then(|sch| sch.idle_for(elapsed_ms(start)))
                    {
                        thread::sleep(rest.min(Duration::from_millis(100)));
                        continue;
                    }
                    let s = round % SEEDS.len();
                    for (k, (name, kernel)) in kernels.iter().enumerate() {
                        let got = black_box(kernel(black_box(SEEDS[s])));
                        if got != expected[k][s] && reported < REPORTED_PER_THREAD {
                            reported += 1;
                            let _ = errors.send(Event::Error {
                                thread: t,
                                cpu,
                                kernel: (*name).to_owned(),
                                elapsed_ms: elapsed_ms(start),
                            });
                        }
                    }
                    counts[t as usize].fetch_add(1, Ordering::Relaxed);
                    round += 1;
                }
            })
        })
        .collect();
    drop(errors);

    let snapshot = |counts: &[AtomicU64]| -> Vec<u64> {
        counts.iter().map(|c| c.load(Ordering::Relaxed)).collect()
    };
    // Errors as they come, progress about once a second.
    let mut next_progress = Duration::from_secs(1);
    while start.elapsed() < params.duration {
        let wait = next_progress
            .min(params.duration)
            .saturating_sub(start.elapsed());
        match inbox.recv_timeout(wait) {
            Ok(event) => writeln!(out, "{}", event.to_line())?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if start.elapsed() >= next_progress {
            let progress = Event::Progress {
                elapsed_ms: elapsed_ms(start),
                iterations: snapshot(&counts),
            };
            writeln!(out, "{}", progress.to_line())?;
            next_progress += Duration::from_secs(1);
        }
        out.flush()?;
    }
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    for event in inbox.try_iter() {
        writeln!(out, "{}", event.to_line())?;
    }
    writeln!(
        out,
        "{}",
        Event::Finished {
            iterations: snapshot(&counts),
        }
        .to_line()
    )?;
    out.flush()
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_cpus_are_listed_where_the_os_says() {
        let cpus = allowed_cpus();
        if cfg!(any(target_os = "linux", windows)) {
            assert!(!cpus.is_empty());
            assert!(cpus.windows(2).all(|w| w[0] < w[1]), "{cpus:?}");
        }
    }

    #[test]
    fn kernels_are_deterministic_and_seed_dependent() {
        for (_, k) in MIXED {
            assert_eq!(k(SEEDS[0]), k(SEEDS[0]));
            assert_ne!(k(SEEDS[0]), k(SEEDS[1]));
        }
    }

    #[test]
    fn short_run_reports_start_progress_and_finish() {
        let mut out = Vec::new();
        run(
            Params {
                threads: 2,
                cpus: Vec::new(),
                duration: Duration::from_millis(1300),
                pattern: Pattern::Steady,
                kernels: Kernels::Mixed,
            },
            &mut out,
        )
        .unwrap();
        let events: Vec<Event> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| Event::parse(l).unwrap())
            .collect();
        assert!(matches!(events[0], Event::Started { threads: 2, .. }));
        assert!(events.iter().any(|e| matches!(e, Event::Progress { .. })));
        assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })));
        let Some(Event::Finished { iterations }) = events.last() else {
            panic!("no finish: {events:?}");
        };
        assert_eq!(iterations.len(), 2);
        assert!(iterations.iter().all(|&n| n > 0));
    }
}
