//! Client mode: a [`Provider`] that asks a running `surtr serve` over local
//! IPC instead of reading the hardware (plan, section 7.2). The daemon answers
//! from its latest read, so clients never disturb its polling.

use std::sync::{Mutex, PoisonError};

use serde::de::DeserializeOwned;
use surtr_backend::{Provider, ProviderError};
use surtr_core::Sample;
use surtr_ipc::Client;
use surtr_model::{
    Capabilities, GpuProcess, SensorDescriptor, SensorReading, SystemInfo, ThrottleState,
};

/// The daemon, seen as one provider.
pub struct RemoteProvider {
    client: Mutex<Client>,
    id: String,
}

impl RemoteProvider {
    /// Connects to the local daemon, if one is running.
    pub fn connect() -> Option<Self> {
        let endpoint = surtr_ipc::endpoint().ok()?;
        let client = Client::connect(&endpoint)
            .inspect_err(|e| {
                // No daemon is the usual case; anything else is worth a note.
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("not using surtr serve: {e}");
                }
            })
            .ok()?;
        let id = format!("{} via surtr serve", client.hello().providers.join(", "));
        Some(Self {
            client: Mutex::new(client),
            id,
        })
    }

    fn call<T: DeserializeOwned>(&self, method: &str) -> Result<T, ProviderError> {
        let value = self
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .call(method)
            .map_err(|reason| ProviderError::Unavailable { reason })?;
        serde_json::from_value(value).map_err(|e| ProviderError::Unavailable {
            reason: format!("unexpected answer from surtr serve: {e}"),
        })
    }
}

impl Provider for RemoteProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> Capabilities {
        self.call("capabilities").unwrap_or_default()
    }

    fn system_info(&self) -> Result<SystemInfo, ProviderError> {
        self.call("system_info")
    }

    fn sensors(&self) -> Result<Vec<SensorDescriptor>, ProviderError> {
        self.call("sensors")
    }

    fn read_sensors(&self) -> Result<Vec<SensorReading>, ProviderError> {
        let samples: Vec<Sample> = self.call("read")?;
        Ok(samples
            .into_iter()
            .filter_map(|s| {
                Some(SensorReading {
                    value: s.value?,
                    at: s.at?,
                    id: s.descriptor.id,
                })
            })
            .collect())
    }

    fn gpu_processes(&self) -> Result<Vec<GpuProcess>, ProviderError> {
        self.call("gpu_processes")
    }

    fn throttle_reasons(&self) -> Result<Vec<ThrottleState>, ProviderError> {
        self.call("throttle_reasons")
    }
}
