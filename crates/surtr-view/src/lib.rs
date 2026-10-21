//! What Surtr shows, independent of how: the CLI, TUI and GUI all build their
//! screens from these view models, labels and theme tokens, so they can't
//! drift apart (plan, sections 4 and 7).
//!
//! Everything here is plain data. A front end decides layout and maps each
//! [`Role`] to its own colors through a [`Theme`].

#![forbid(unsafe_code)]

pub mod format;
pub mod health;
pub mod status;
pub mod theme;
mod view;

pub use health::Level;
pub use status::{Finding, Status, status};
pub use theme::{Rgb, Role, Theme};
pub use view::{
    Cell, Row, Section, capability_rows, process_rows, sensor_sections, system_sections,
    throttle_rows,
};
