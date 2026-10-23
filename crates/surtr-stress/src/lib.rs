//! Built-in stress tests (plan, section 5.3).
//!
//! Each test runs in a worker process (the `surtr` binary started again as a
//! hidden `surtr worker`), so a crash, hang or out-of-memory kill never takes
//! the host down and the host can always stop it. The worker reports through
//! the run-event schema in [`event`]; the host side is [`runner`]. The
//! protocol is described in `docs/stress-protocol.md`.

use std::{io, io::Write, time::Duration};

use serde::Serialize;

pub mod compare;
pub mod cpu;
pub mod event;
pub mod html;
pub mod record;
pub mod request;
pub mod runner;
pub mod topology;

/// How a workload must be run (plan, section 4, the isolation rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Always in a worker process: every stress and verification test, since
    /// a crash or hang is a result the host must survive and record.
    Required,
    /// May run in the host: short, low-risk benchmarks (none yet).
    Optional,
}

/// What a workload is asked to do.
#[derive(Debug, Clone)]
pub struct Params {
    /// Worker threads; one per logical CPU is the usual choice. Ignored
    /// when `cpus` is given.
    pub threads: u32,
    /// The logical CPU for each thread, as the host placed them
    /// ([`topology::placement`]); empty means the worker decides.
    pub cpus: Vec<u32>,
    /// How long to run.
    pub duration: Duration,
}

/// A built-in stress test. It runs inside the worker and writes run events
/// ([`event::Event`], one JSON line each) to `out` until its duration has
/// passed; the host stops it earlier by killing the worker.
pub trait Workload: Send + Sync {
    /// Short name, as in `surtr stress <name>`.
    fn name(&self) -> &'static str;
    /// One sentence on what it does.
    fn description(&self) -> &'static str;
    /// How it must be run.
    fn isolation(&self) -> Isolation;
    /// Runs it.
    fn run(&self, params: Params, out: &mut dyn Write) -> io::Result<()>;
}

/// Every built-in workload.
pub fn workloads() -> Vec<Box<dyn Workload>> {
    vec![Box::new(cpu::Cpu)]
}

/// The built-in workload called `name`.
pub fn workload(name: &str) -> Option<Box<dyn Workload>> {
    workloads().into_iter().find(|w| w.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry() {
        let cpu = workload("cpu").unwrap();
        assert_eq!(cpu.isolation(), Isolation::Required);
        assert!(workload("nope").is_none());
        // Every stress test needs a worker.
        assert!(
            workloads()
                .iter()
                .all(|w| w.isolation() == Isolation::Required)
        );
    }
}
