//! The [`Provider`] trait that every Surtr hardware backend implements, and a
//! deterministic [`mock`] provider (feature `mock`).
//!
//! This crate holds no OS or vendor code. Per-OS and per-vendor backends live
//! in their own crates and depend on this one; backends never depend on each
//! other (plan, sections 4 and 4.1).

#![forbid(unsafe_code)]

use std::{error::Error, fmt, io};

use surtr_model::{
    Capabilities, GpuProcess, SensorDescriptor, SensorReading, SystemInfo, ThrottleState,
};

#[cfg(any(test, feature = "mock"))]
pub mod mock;

/// A source of hardware information and sensor readings.
///
/// Implementations fill only what they can actually read and tag every value
/// with its [`surtr_model::Source`]; everything else stays absent. Methods take
/// `&self` so a provider can be shared between threads; a provider that needs
/// mutable state uses interior mutability.
pub trait Provider: Send + Sync {
    /// Short, stable identifier, used as [`surtr_model::Source::provider`].
    fn id(&self) -> &str;

    /// What this provider can and cannot do on this machine.
    fn capabilities(&self) -> Capabilities;

    /// A hardware and OS snapshot. Cheap enough to call on demand, but not
    /// meant for every poll.
    fn system_info(&self) -> Result<SystemInfo, ProviderError>;

    /// The sensors this provider offers. Stable between calls unless hardware
    /// changes.
    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError>;

    /// Reads all sensors once. Sensors that can't be read this time are left
    /// out rather than reported as zero.
    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError>;

    /// Processes using this provider's GPUs. Providers without GPUs keep the
    /// default (none). Process names may be left for the caller to fill in.
    fn gpu_processes(&self) -> Result<Vec<GpuProcess>, ProviderError> {
        Ok(Vec::new())
    }

    /// Why each of this provider's devices is below its potential clock.
    /// Providers that can't tell keep the default (no devices).
    fn throttle_reasons(&self) -> Result<Vec<ThrottleState>, ProviderError> {
        Ok(Vec::new())
    }
}

/// Why a provider call failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProviderError {
    /// The provider can't work here (missing library, driver or hardware).
    Unavailable {
        /// Short explanation shown to the user.
        reason: String,
    },
    /// The OS refused access; elevation or the privileged backend may help.
    PermissionDenied {
        /// Short explanation shown to the user.
        reason: String,
    },
    /// An I/O error from the OS.
    Io(io::Error),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { reason } => write!(f, "provider unavailable: {reason}"),
            Self::PermissionDenied { reason } => write!(f, "permission denied: {reason}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl Error for ProviderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for ProviderError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
