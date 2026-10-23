//! Built-in stress tests (plan, section 5.3).
//!
//! Each test runs in a worker process (the `surtr` binary started again as a
//! hidden `surtr worker`), so a crash, hang or out-of-memory kill never takes
//! the host down and the host can always stop it. The worker reports through
//! the run-event schema in [`event`]; the host side is [`runner`].

pub mod cpu;
pub mod event;
pub mod html;
pub mod record;
pub mod request;
pub mod runner;
