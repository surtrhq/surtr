use serde::{Deserialize, Serialize};

use crate::{Field, Source};

/// A process using a GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuProcess {
    /// Device it runs on, for example `"nvidia0"`.
    pub device: String,
    /// OS process id.
    pub pid: u32,
    /// Executable name without its path, if the OS reveals it.
    pub name: Option<String>,
    /// How it uses the GPU.
    pub kind: GpuProcessKind,
    /// GPU memory it holds, in bytes. Often absent on Windows, where the
    /// driver model hides per-process usage.
    pub used_memory_bytes: Field<u64>,
}

/// How a process uses a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuProcessKind {
    /// Compute (CUDA, OpenCL and similar).
    Compute,
    /// Graphics (3D rendering, display).
    Graphics,
    /// The driver doesn't say (for example Linux DRM `fdinfo`).
    Unknown,
}

/// Why a GPU runs below its potential clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ThrottleReason {
    /// Nothing is running, so clocks are low on purpose.
    Idle,
    /// Clocks are held by an application or administrator setting.
    ApplicationClocks,
    /// The power limit is reached.
    PowerCap,
    /// Hardware slowdown (thermal or power emergency).
    HardwareSlowdown,
    /// Clocks are synchronized with other GPUs.
    SyncBoost,
    /// The driver or GPU firmware lowers clocks to stay under a temperature
    /// limit.
    SoftwareThermal,
    /// Hardware thermal protection is active.
    HardwareThermal,
    /// An external power brake signal is active.
    HardwarePowerBrake,
    /// Clocks follow a display clock setting.
    DisplayClocks,
    /// A current limit (TDC or EDC) is reached.
    CurrentLimit,
    /// The device reports a limit Surtr doesn't classify.
    Other,
}

impl ThrottleReason {
    /// Every reason, in display order.
    pub const ALL: [ThrottleReason; 11] = [
        Self::Idle,
        Self::ApplicationClocks,
        Self::PowerCap,
        Self::HardwareSlowdown,
        Self::SyncBoost,
        Self::SoftwareThermal,
        Self::HardwareThermal,
        Self::HardwarePowerBrake,
        Self::DisplayClocks,
        Self::CurrentLimit,
        Self::Other,
    ];
}

/// The throttle reasons active on one device right now. An empty list means
/// the device runs unrestricted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThrottleState {
    /// Device, for example `"nvidia0"`.
    pub device: String,
    /// Active reasons.
    pub reasons: Vec<ThrottleReason>,
    /// Where the information came from.
    pub source: Source,
}
