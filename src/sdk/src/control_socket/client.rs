//! The control socket client, used by the MCP shim to reach a running Medulla.
//!
//! One connection carries the whole harness session, with requests correlated by
//! an increasing id. The shim is a subprocess of the ACP agent, not of Medulla,
//! so it must survive Medulla being absent: every failure here is a value the
//! caller can turn into a readable tool refusal rather than a reason to die.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::time::{timeout_at, Instant};

use super::types::{ControlError, ControlFailure, ErrorKind, Hello, PROTOCOL_VERSION};

/// A connected, authenticated control session.
pub struct ControlClient {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    hello: Hello,
    usable: bool,
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
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = timeout_at(deadline, UnixStream::connect(path))
            .await
            .map_err(|_| ControlError::Transport("timed out connecting to Medulla".into()))?
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
            usable: true,
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
            .call_until(
                "hello",
                json!({ "protocol": PROTOCOL_VERSION, "token": token }),
                deadline,
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
        self.call_until(op, params, Instant::now() + Duration::from_secs(125))
            .await
    }

    /// Tell the server how a workflow run this process is executing is going.
    ///
    /// Best-effort by construction: the caller is executing the run either way,
    /// and a control plane that has gone away is a lost *view* of the work
    /// rather than a lost run. Callers therefore drop the error.
    ///
    /// # Errors
    ///
    /// Whatever [`call`](Self::call) reports — a dropped connection, or a
    /// refusal from a server whose grant has since been revoked.
    pub async fn report_run(
        &mut self,
        run_id: &str,
        workflow_id: &str,
        status: &str,
        detail: Option<&str>,
        node: Option<&str>,
    ) -> Result<Value, ControlError> {
        self.call(
            "run.report",
            json!({
                "runId": run_id,
                "workflowId": workflow_id,
                "status": status,
                "detail": detail,
                "node": node,
            }),
        )
        .await
    }

    /// Issue one op, refusing to wait past the caller's deadline.
    ///
    /// Fleet polling supplies a deadline derived from its requested wait;
    /// shorter operations use a small fixed budget. This keeps a silent peer
    /// from wedging the MCP server while still permitting deliberate long
    /// polls.
    ///
    /// # Errors
    ///
    /// [`ControlError::Disconnected`] when the deadline expires or the stream
    /// ends mid-call.
    pub async fn call_until(
        &mut self,
        op: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, ControlError> {
        if !self.usable {
            return Err(ControlError::Disconnected(
                "the control connection is no longer usable; reconnect before retrying".into(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({ "v": PROTOCOL_VERSION, "id": id, "op": op, "params": params });

        let write_result = timeout_at(
            deadline,
            self.write.write_all(format!("{frame}\n").as_bytes()),
        )
        .await;
        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.usable = false;
                return Err(ControlError::Disconnected(error.to_string()));
            }
            Err(_) => {
                self.usable = false;
                return Err(ControlError::Disconnected(
                    "timed out writing the request".into(),
                ));
            }
        }
        match timeout_at(deadline, self.write.flush()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.usable = false;
                return Err(ControlError::Disconnected(error.to_string()));
            }
            Err(_) => {
                self.usable = false;
                return Err(ControlError::Disconnected(
                    "timed out flushing the request".into(),
                ));
            }
        }

        loop {
            let line = match timeout_at(deadline, self.lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => {
                    self.usable = false;
                    return Err(ControlError::Disconnected(
                        "the server closed the connection".to_string(),
                    ));
                }
                Ok(Err(error)) => {
                    self.usable = false;
                    return Err(ControlError::Disconnected(error.to_string()));
                }
                Err(_) => {
                    self.usable = false;
                    return Err(ControlError::Disconnected(
                        "timed out waiting for Medulla's reply".into(),
                    ));
                }
            };
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
