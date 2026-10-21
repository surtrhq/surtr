//! Plain, typed data for Surtr: hardware descriptions, sensors and
//! capabilities. No I/O and no OS-specific code.
//!
//! Every value that comes from hardware is a [`Field`]: either absent (not
//! known) or a value tagged with the [`Source`] it came from. Absent never
//! means zero or a default; front ends show it as "not available".
//!
//! By design the model has no place for identifiers such as serial numbers,
//! MAC addresses or host names (plan, section 6.1).

#![forbid(unsafe_code)]

mod capability;
mod gpu;
mod sensor;
mod source;
mod system;

pub use capability::{Capabilities, Capability, Feature, Support};
pub use gpu::{GpuProcess, GpuProcessKind, ThrottleReason, ThrottleState};
pub use sensor::{SensorDescriptor, SensorId, SensorKind, SensorReading, Timestamp, Unit};
pub use source::{Field, Source, Sourced};
pub use system::{
    Arch, CpuInfo, CpuVendor, GpuInfo, GpuVendor, MemoryInfo, MemoryModule, OsInfo, SystemInfo,
};
