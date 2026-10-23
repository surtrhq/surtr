//! The run-event schema: what a worker reports to the host, one JSON object
//! per line on its standard output. Built-in tests and (later) plugins use
//! the same events, so the host treats them alike (plan, section 4).

use serde::{Deserialize, Serialize};

/// Version of the event schema; workers state it in [`Event::Started`].
pub const SCHEMA: u32 = 1;

/// One event from a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The workload started.
    Started {
        /// Schema version, [`SCHEMA`].
        schema: u32,
        /// Workload name, for example `cpu`.
        workload: String,
        /// Worker threads.
        threads: u32,
        /// The computations it runs, for example `fma-avx2`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        kernels: Vec<String>,
    },
    /// Completed iterations per thread so far, about once a second.
    Progress {
        /// Milliseconds since the start.
        elapsed_ms: u64,
        /// Iterations each thread has completed.
        iterations: Vec<u64>,
    },
    /// A computation gave a wrong answer: the hardware made a mistake.
    Error {
        /// Worker thread, from 0.
        thread: u32,
        /// Logical CPU the thread was pinned to, if it was.
        cpu: Option<u32>,
        /// Which computation failed.
        kernel: String,
        /// Milliseconds since the start.
        elapsed_ms: u64,
    },
    /// The workload ran for its whole duration.
    Finished {
        /// Iterations each thread completed.
        iterations: Vec<u64>,
    },
}

impl Event {
    /// The event as one line of JSON, without the newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses one line; anything else (a panic message, say) is `None`.
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_trip() {
        let started = Event::Started {
            schema: SCHEMA,
            workload: "cpu".into(),
            threads: 2,
            kernels: Vec::new(),
        };
        // Older lines without kernels still parse.
        assert_eq!(
            Event::parse(r#"{"event":"started","schema":1,"workload":"cpu","threads":2}"#),
            Some(started)
        );
        let e = Event::Error {
            thread: 3,
            cpu: Some(3),
            kernel: "integer".into(),
            elapsed_ms: 1500,
        };
        let line = e.to_line();
        assert!(line.starts_with(r#"{"event":"error","#), "{line}");
        assert_eq!(Event::parse(&line), Some(e));
        assert_eq!(Event::parse("thread 'main' panicked"), None);
    }
}
