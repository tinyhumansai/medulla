//! The fleet reached over the control socket.
//!
//! Each call uses its own authenticated connection. A long `task.get` poll must
//! not hold the route an urgent `task.abort` needs to reach the same server.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::control_socket::{ControlClient, ControlError, Hello, MAX_WAIT};

use super::FleetBackend;

/// Margin above a server-side long poll before the client gives up.
const REPLY_MARGIN: Duration = Duration::from_secs(5);
/// Deadline for operations that do not intentionally wait on task progress.
const SHORT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// The matching control-server ceiling for `task.get` long polls.
const MAX_WAIT_SECONDS: u64 = MAX_WAIT.as_secs();

/// A fleet reached through a running Medulla's control socket.
pub struct ProxyFleet {
    path: PathBuf,
    token: String,
    hello: Hello,
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
        })
    }

    /// Choose a response deadline that preserves the requested server wait.
    fn deadline(op: &str, params: &Value) -> Instant {
        let timeout = if op == "task.get" {
            params
                .get("waitSeconds")
                .and_then(Value::as_u64)
                .map(|seconds| Duration::from_secs(seconds.min(MAX_WAIT_SECONDS)))
                .unwrap_or_default()
                .saturating_add(REPLY_MARGIN)
                .max(SHORT_REPLY_TIMEOUT)
        } else {
            SHORT_REPLY_TIMEOUT
        };
        Instant::now() + timeout
    }

    /// Preserve the ambiguity of a disconnected mutating request.
    fn disconnected(op: &str, reason: String) -> ControlError {
        if matches!(op, "worker.list" | "task.get" | "task.list") {
            ControlError::Disconnected(reason)
        } else {
            ControlError::Disconnected(format!(
                "{reason} — this request may or may not have been accepted; \
                 call fleet_tasks to see what is running before retrying"
            ))
        }
    }
}

#[async_trait::async_trait]
impl FleetBackend for ProxyFleet {
    fn hello(&self) -> Option<&Hello> {
        Some(&self.hello)
    }

    #[cfg(unix)]
    async fn call(&self, op: &str, params: Value) -> Result<Value, ControlError> {
        let mut client = ControlClient::connect(&self.path, &self.token).await?;
        client
            .call_until(op, params.clone(), Self::deadline(op, &params))
            .await
            .map_err(|error| match error {
                ControlError::Disconnected(reason) => Self::disconnected(op, reason),
                other => other,
            })
    }

    #[cfg(not(unix))]
    async fn call(&self, _op: &str, _params: Value) -> Result<Value, ControlError> {
        Err(ControlError::NoInstance)
    }
}
