//! The control socket client, used by the MCP shim to reach a running Medulla.
//!
//! One connection carries the whole harness session, with requests correlated by
//! an increasing id. The shim is a subprocess of the ACP agent, not of Medulla,
//! so it must survive Medulla being absent: every failure here is a value the
//! caller can turn into a readable tool refusal rather than a reason to die.

use std::path::Path;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;

use super::types::{ControlError, ControlFailure, ErrorKind, Hello, PROTOCOL_VERSION};

/// A connected, authenticated control session.
pub struct ControlClient {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    hello: Hello,
}

impl ControlClient {
    /// Connect to `path` and complete the `hello` handshake with `token`.
    ///
    /// # Errors
    ///
    /// [`ControlError::NoInstance`] when nothing is listening — the ordinary
    /// case on a host whose spawn carried no grant, and never worth failing a
    /// session over. [`ControlError::Refused`] when the grant is not valid.
    #[cfg(unix)]
    pub async fn connect(path: &Path, token: &str) -> Result<Self, ControlError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    ControlError::NoInstance
                }
                _ => ControlError::Transport(err.to_string()),
            })?;
        let (read_half, write) = stream.into_split();
        let mut client = ControlClient {
            lines: BufReader::new(read_half).lines(),
            write,
            next_id: 1,
            // Replaced by the handshake below; a placeholder rather than an
            // `Option` so every later read is unconditional.
            hello: Hello {
                protocol: PROTOCOL_VERSION,
                version: String::new(),
                hub_ready: false,
                depth: 0,
                max_depth: 0,
                max_in_flight: 0,
                families: super::types::ToolFamilies {
                    workflows: true,
                    fleet: false,
                },
            },
        };
        let result = client
            .call(
                "hello",
                json!({ "protocol": PROTOCOL_VERSION, "token": token }),
            )
            .await?;
        client.hello = serde_json::from_value(result)
            .map_err(|err| ControlError::Transport(format!("malformed hello: {err}")))?;
        Ok(client)
    }

    /// What the server said about itself and this grant.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Issue one op and await its correlated reply.
    ///
    /// # Errors
    ///
    /// [`ControlError::Disconnected`] when the stream ends mid-call, which the
    /// caller distinguishes from a refusal because it is the one failure worth
    /// reconnecting for.
    pub async fn call(&mut self, op: &str, params: Value) -> Result<Value, ControlError> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({ "v": PROTOCOL_VERSION, "id": id, "op": op, "params": params });

        self.write
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .map_err(|err| ControlError::Disconnected(err.to_string()))?;
        self.write
            .flush()
            .await
            .map_err(|err| ControlError::Disconnected(err.to_string()))?;

        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|err| ControlError::Disconnected(err.to_string()))?
                .ok_or_else(|| {
                    ControlError::Disconnected("the server closed the connection".to_string())
                })?;
            if line.trim().is_empty() {
                continue;
            }
            let frame: Value = serde_json::from_str(&line)
                .map_err(|err| ControlError::Transport(format!("malformed frame: {err}")))?;
            // Skip anything that is not the answer to this call. Nothing sends
            // unsolicited frames today, but a client that assumed so would break
            // the first time one did.
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if frame.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
            }
            let error = frame.get("error").cloned().unwrap_or(Value::Null);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the control plane refused the request")
                .to_string();
            let kind = match error.get("kind").and_then(Value::as_str) {
                Some("unauthenticated") => ErrorKind::Unauthenticated,
                Some("versionMismatch") => ErrorKind::VersionMismatch,
                Some("hubNotReady") => ErrorKind::HubNotReady,
                Some("noSuchWorker") => ErrorKind::NoSuchWorker,
                Some("noSuchTask") => ErrorKind::NoSuchTask,
                Some("depthExceeded") => ErrorKind::DepthExceeded,
                Some("tooManyInFlight") => ErrorKind::TooManyInFlight,
                Some("dispatchFailed") => ErrorKind::DispatchFailed,
                Some("badRequest") => ErrorKind::BadRequest,
                _ => ErrorKind::Internal,
            };
            return Err(ControlError::Refused(ControlFailure::new(kind, message)));
        }
    }
}
