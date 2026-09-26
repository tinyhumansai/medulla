//! Newline framing and lifecycle for one accepted control connection.
//!
//! The listener owns binding and acceptance; this module owns the bounded
//! handshake, request framing, handler calls, and response writes after accept.

use std::sync::Arc;

use serde_json::Value;

use super::handle::handle_control;
use super::types::{SessionState, TaskRegistry};
use crate::control_socket::grants::GrantRegistry;
use crate::control_socket::runs::HarnessRunRegistry;
use crate::control_socket::types::FleetOps;

/// The largest frame the server will read.
///
/// An instruction can legitimately be long — it is a whole brief for an agent
/// with no context — so this is generous rather than tight. Its job is to stop
/// an endless line from consuming memory, not to police content.
pub(super) const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long a connection has to complete `hello`.
///
/// An authenticated shim may remain idle for its full harness session, so this
/// bound applies only before authentication.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Serve one connection until it closes or the server shuts down.
#[cfg(unix)]
pub(super) async fn serve_connection(
    stream: tokio::net::UnixStream,
    ops: Arc<dyn FleetOps>,
    grants: GrantRegistry,
    registry: TaskRegistry,
    runs: HarnessRunRegistry,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    serve_connection_with_handshake_timeout(
        stream,
        ops,
        grants,
        registry,
        runs,
        shutdown,
        HANDSHAKE_TIMEOUT,
    )
    .await
}

/// Serve a connection with an explicit pre-authentication lifetime.
///
/// Kept separate so the absolute-deadline behavior can be tested without
/// making the suite wait for the production five-second bound.
#[cfg(unix)]
pub(super) async fn serve_connection_with_handshake_timeout(
    stream: tokio::net::UnixStream,
    ops: Arc<dyn FleetOps>,
    grants: GrantRegistry,
    registry: TaskRegistry,
    runs: HarnessRunRegistry,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    handshake_timeout: std::time::Duration,
) -> std::io::Result<()> {
    use tokio::io::{AsyncWriteExt, BufReader};

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut session = SessionState::default();
    // One lifetime for the unauthenticated connection, not one per frame. A
    // client sending blank lines must not be able to renew its socket forever.
    let handshake_deadline = tokio::time::Instant::now() + handshake_timeout;

    loop {
        // The handshake is bounded; an authenticated connection is not, because
        // a shim legitimately sits idle between a harness's tool calls. Either
        // way the read races shutdown so no connection outlives its server.
        let next = if session.grant().is_none() {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                read = tokio::time::timeout_at(handshake_deadline, read_frame(&mut reader)) => match read {
                    Ok(next) => next?,
                    Err(_) => return Ok(()),
                },
            }
        } else {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                read = read_frame(&mut reader) => read?,
            }
        };
        let Some(line) = next else {
            return Ok(());
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                handle_control(&ops, &grants, &registry, &runs, &mut session, &request).await
            }
            Err(err) => serde_json::json!({
                "v": crate::control_socket::types::PROTOCOL_VERSION,
                "id": Value::Null,
                "ok": false,
                "error": {
                    "kind": crate::control_socket::types::ErrorKind::BadRequest.as_wire(),
                    "message": format!("could not parse frame: {err}"),
                    "retryable": false,
                },
            }),
        };
        write_half
            .write_all(format!("{response}\n").as_bytes())
            .await?;
        write_half.flush().await?;
    }
}

/// Read one newline-delimited frame without ever accumulating an endless line.
///
/// `None` means EOF or an oversized frame; both close the connection. The
/// per-call `take` limit applies before `read_until` buffers bytes.
#[cfg(unix)]
pub(super) async fn read_frame<R>(reader: &mut R) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let mut bytes = Vec::with_capacity(4096);
    let read = reader
        .take((MAX_FRAME_BYTES + 2) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}
