//! The fleet reached over the control socket.
//!
//! One connection serves the whole harness session. It is held behind a mutex
//! because the control protocol correlates replies per connection and every op
//! is short — a dispatch returns a handle rather than waiting for the task, so
//! serialising calls costs nothing worth the complexity of a second connection.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::control_socket::{ControlClient, ControlError, Hello};

use super::FleetBackend;

/// Ops that may safely be sent again after a dropped connection.
///
/// `task.dispatch` is deliberately absent. A retried dispatch whose first
/// attempt was accepted runs the work twice, and the caller cannot tell — so it
/// is reported honestly instead, and the caller checks `fleet_tasks`.
const IDEMPOTENT_OPS: [&str; 3] = ["worker.list", "task.get", "task.list"];

/// A fleet reached through a running Medulla's control socket.
pub struct ProxyFleet {
    path: PathBuf,
    token: String,
    hello: Hello,
    client: Mutex<Option<ControlClient>>,
}

impl ProxyFleet {
    /// Connect and complete the handshake.
    ///
    /// # Errors
    ///
    /// Whatever [`ControlClient::connect`] refused with — most often
    /// [`ControlError::NoInstance`], which the caller turns into the offline
    /// backend rather than a failure.
    #[cfg(unix)]
    pub async fn connect(path: &Path, token: &str) -> Result<Self, ControlError> {
        let client = ControlClient::connect(path, token).await?;
        Ok(ProxyFleet {
            path: path.to_path_buf(),
            token: token.to_string(),
            hello: client.hello().clone(),
            client: Mutex::new(Some(client)),
        })
    }
}

#[async_trait::async_trait]
impl FleetBackend for ProxyFleet {
    fn hello(&self) -> Option<&Hello> {
        Some(&self.hello)
    }

    #[cfg(unix)]
    async fn call(&self, op: &str, params: Value) -> Result<Value, ControlError> {
        let mut slot = self.client.lock().await;

        if let Some(client) = slot.as_mut() {
            match client.call(op, params.clone()).await {
                Ok(result) => return Ok(result),
                Err(ControlError::Disconnected(reason)) => {
                    // The connection is unusable either way; drop it so the
                    // reconnect below is the only path forward.
                    *slot = None;
                    if !IDEMPOTENT_OPS.contains(&op) {
                        return Err(ControlError::Disconnected(format!(
                            "{reason} — this request may or may not have been accepted; \
                             call fleet_tasks to see what is running before retrying"
                        )));
                    }
                }
                Err(other) => return Err(other),
            }
        }

        let mut client = ControlClient::connect(&self.path, &self.token).await?;
        let result = client.call(op, params).await;
        *slot = Some(client);
        result
    }

    #[cfg(not(unix))]
    async fn call(&self, _op: &str, _params: Value) -> Result<Value, ControlError> {
        Err(ControlError::NoInstance)
    }
}
