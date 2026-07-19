//! The connected [`CoreClient`] and its NDJSON transport: connect/split the Unix
//! socket, correlate `{id, ok|error}` responses to awaiting `request`s, and drive
//! the background read loop that forwards unsolicited `{"t":"event"}` frames to the
//! events channel (§1–§3 of docs/unified-app/04-protocol-contract.md).
//!
//! The client owns one `tokio::net::UnixStream`, split into a write half (behind a
//! mutex, used by [`CoreClient::request`]) and a read half driven by
//! [`read_loop`]. Frames are newline-delimited JSON with a 1 MiB cap
//! ([`MAX_FRAME_BYTES`], §1.1); an over-size frame is a protocol error, never a
//! truncation. `serde_json` handles (de)serialization — no hand-rolled JSON.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};

use super::types::{CallError, CoreEvent, RpcError, MAX_FRAME_BYTES, PROTOCOL_VERSION};

/// The shared registry of in-flight requests, keyed by frame id, each awaiting its
/// correlated response over a `oneshot`.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// A connected core client.
pub struct CoreClient {
    socket_path: std::path::PathBuf,
    write: Arc<Mutex<OwnedWriteHalf>>,
    next_id: AtomicU64,
    pending: PendingMap,
}

impl CoreClient {
    /// Connect to the core singleton and start the read task. Returns the client and
    /// the receiver of unsolicited event frames (§3.2).
    pub async fn connect(
        socket_path: &std::path::Path,
    ) -> std::io::Result<(CoreClient, mpsc::UnboundedReceiver<CoreEvent>)> {
        let stream = UnixStream::connect(socket_path).await?;
        let (read_half, write_half) = stream.into_split();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let reader_pending = pending.clone();
        tokio::spawn(async move {
            read_loop(read_half, reader_pending, events_tx).await;
        });

        let client = CoreClient {
            socket_path: socket_path.to_path_buf(),
            write: Arc::new(Mutex::new(write_half)),
            next_id: AtomicU64::new(1),
            pending,
        };
        Ok((client, events_rx))
    }

    /// The Unix socket path this client is connected to.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// Send a request and await its correlated response (§2). RPC errors surface as
    /// `Err(CallError::Rpc)` with their `data` intact; transport failures as
    /// `CallError::Transport`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let frame = json!({ "id": id, "method": method, "params": params });
        let mut line =
            serde_json::to_vec(&frame).map_err(|e| CallError::Transport(e.to_string()))?;
        line.push(b'\n');
        if line.len() > MAX_FRAME_BYTES {
            self.pending.lock().await.remove(&id);
            return Err(CallError::Transport(
                "outbound frame exceeds 1 MiB cap".into(),
            ));
        }
        {
            let mut w = self.write.lock().await;
            w.write_all(&line)
                .await
                .map_err(|e| CallError::Transport(e.to_string()))?;
            w.flush()
                .await
                .map_err(|e| CallError::Transport(e.to_string()))?;
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CallError::Rpc(e)),
            Err(_) => Err(CallError::Transport(
                "core closed the connection before responding".into(),
            )),
        }
    }

    // --- typed methods (§2 table) -----------------------------------------------

    /// The `initialize` handshake / liveness probe (§2). When `memory` is set,
    /// it advertises the persona-memory capability (tool names + pack path) so
    /// the reasoning layer may issue `memory_query` events.
    pub async fn initialize(
        &self,
        client_version: &str,
        memory: Option<Value>,
    ) -> Result<Value, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("clientVersion".into(), json!(client_version));
        params.insert("protocolVersion".into(), json!(PROTOCOL_VERSION));
        if let Some(capability) = memory {
            params.insert("memory".into(), capability);
        }
        self.request("initialize", Value::Object(params)).await
    }

    /// `thread.list` — enumerate the core's threads.
    pub async fn thread_list(&self) -> Result<Value, CallError> {
        self.request("thread.list", json!({})).await
    }

    /// `thread.create` — create a thread, returning its new `threadId`.
    pub async fn thread_create(
        &self,
        name: Option<&str>,
        surface: Option<&str>,
    ) -> Result<String, CallError> {
        let mut params = serde_json::Map::new();
        if let Some(n) = name {
            params.insert("name".into(), json!(n));
        }
        if let Some(s) = surface {
            params.insert("surface".into(), json!(s));
        }
        let ok = self.request("thread.create", Value::Object(params)).await?;
        ok.get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CallError::Transport("thread.create returned no threadId".into()))
    }

    /// `thread.resume` — reattach to an existing thread.
    pub async fn thread_resume(&self, thread_id: &str) -> Result<Value, CallError> {
        self.request("thread.resume", json!({ "threadId": thread_id }))
            .await
    }

    /// `thread.fork` — branch a thread (optionally at `at_seq`), returning the new
    /// `threadId`.
    pub async fn thread_fork(
        &self,
        thread_id: &str,
        at_seq: Option<u64>,
    ) -> Result<String, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        if let Some(s) = at_seq {
            params.insert("atSeq".into(), json!(s));
        }
        let ok = self.request("thread.fork", Value::Object(params)).await?;
        ok.get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CallError::Transport("thread.fork returned no threadId".into()))
    }

    /// `thread.subscribe` — register the go-forward tap and get `{baselineSeq,
    /// resync?, snapshot?}` (§3.2). Events then arrive on the [`connect`](CoreClient::connect) receiver.
    pub async fn thread_subscribe(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
    ) -> Result<Value, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        if let Some(s) = since_seq {
            params.insert("sinceSeq".into(), json!(s));
        }
        self.request("thread.subscribe", Value::Object(params))
            .await
    }

    /// `cycle.submit` — returns the `cycleId` receipt (§2.1), not the reply.
    pub async fn cycle_submit(
        &self,
        thread_id: &str,
        input: &str,
        config: Option<Value>,
    ) -> Result<String, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        params.insert("input".into(), json!(input));
        if let Some(c) = config {
            params.insert("config".into(), c);
        }
        let ok = self.request("cycle.submit", Value::Object(params)).await?;
        ok.get("cycleId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CallError::Transport("cycle.submit returned no cycleId".into()))
    }

    /// `cycle.abort` — stop an in-flight cycle.
    pub async fn cycle_abort(&self, cycle_id: &str) -> Result<Value, CallError> {
        self.request("cycle.abort", json!({ "cycleId": cycle_id }))
            .await
    }

    /// `task.cancel` — cancel one task lane within a cycle.
    pub async fn task_cancel(&self, cycle_id: &str, task_id: &str) -> Result<Value, CallError> {
        self.request(
            "task.cancel",
            json!({ "cycleId": cycle_id, "taskId": task_id }),
        )
        .await
    }

    /// `question.answer` — reply to a pending `task_attention` question.
    pub async fn question_answer(
        &self,
        cycle_id: &str,
        question_id: &str,
        body: &str,
    ) -> Result<Value, CallError> {
        self.request(
            "question.answer",
            json!({ "cycleId": cycle_id, "questionId": question_id, "body": body }),
        )
        .await
    }

    /// Answer a core-issued `memory_query` event by its `id`. Exactly one of
    /// `ok` / `error` should be set: `memory.answer` mirrors `question.answer`,
    /// serving a memory tool result (or an error) back to the reasoning layer.
    pub async fn memory_answer(
        &self,
        id: &str,
        ok: Option<Value>,
        error: Option<&str>,
    ) -> Result<Value, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("id".into(), json!(id));
        if let Some(result) = ok {
            params.insert("ok".into(), result);
        }
        if let Some(message) = error {
            params.insert("error".into(), json!({ "message": message }));
        }
        self.request("memory.answer", Value::Object(params)).await
    }

    /// `snapshot.get` — the resync path (§3.4/§6). A `resync.required` RPC error still
    /// carries the durable snapshot in `data`, so this returns it in both cases: on
    /// success from `ok.snapshot`, on `resync.required` from `error.data.snapshot`.
    pub async fn snapshot_get(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
    ) -> Result<Value, CallError> {
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), json!(thread_id));
        if let Some(s) = since_seq {
            params.insert("sinceSeq".into(), json!(s));
        }
        match self.request("snapshot.get", Value::Object(params)).await {
            Ok(v) => Ok(v),
            Err(CallError::Rpc(e)) if e.code == "resync.required" => {
                // The incremental replay is gone, but the folded snapshot rode in
                // `data`; hand it back as an ok-shaped payload for the caller to fold.
                let data = e.data.clone().unwrap_or_else(|| json!({}));
                Ok(data)
            }
            Err(e) => Err(e),
        }
    }

    /// `roster.list` — the delegatable-agent roster.
    pub async fn roster_list(&self) -> Result<Value, CallError> {
        self.request("roster.list", json!({})).await
    }

    /// `context.inspect` — the assembled context for a cycle.
    pub async fn context_inspect(&self, cycle_id: &str) -> Result<Value, CallError> {
        self.request("context.inspect", json!({ "cycleId": cycle_id }))
            .await
    }

    /// `config.set` — apply a config patch to a thread.
    pub async fn config_set(&self, thread_id: &str, patch: Value) -> Result<Value, CallError> {
        self.request(
            "config.set",
            json!({ "threadId": thread_id, "patch": patch }),
        )
        .await
    }

    // --- worker.* (managed remote peers) ----------------------------------------

    /// `worker.list` — the managed worker-peer registry.
    pub async fn worker_list(&self) -> Result<Value, CallError> {
        self.request("worker.list", json!({})).await
    }

    /// `worker.add` — register a new worker peer.
    pub async fn worker_add(
        &self,
        address: Option<&str>,
        handle: Option<&str>,
        label: Option<&str>,
        harness: Option<&str>,
    ) -> Result<Value, CallError> {
        let mut params = serde_json::Map::new();
        if let Some(a) = address {
            params.insert("address".into(), json!(a));
        }
        if let Some(h) = handle {
            params.insert("handle".into(), json!(h));
        }
        if let Some(l) = label {
            params.insert("label".into(), json!(l));
        }
        if let Some(h) = harness {
            params.insert("harness".into(), json!(h));
        }
        self.request("worker.add", Value::Object(params)).await
    }

    /// `worker.update` — patch fields on a registered worker peer.
    pub async fn worker_update(&self, id: &str, patch: Value) -> Result<Value, CallError> {
        self.request("worker.update", json!({ "id": id, "patch": patch }))
            .await
    }

    /// `worker.remove` — deregister a worker peer.
    pub async fn worker_remove(&self, id: &str) -> Result<Value, CallError> {
        self.request("worker.remove", json!({ "id": id })).await
    }

    /// `worker.select` — mark a worker peer as the active delegation target.
    pub async fn worker_select(&self, id: &str) -> Result<Value, CallError> {
        self.request("worker.select", json!({ "id": id })).await
    }
}

/// The background read loop: split responses from event frames, routing each.
async fn read_loop(
    read_half: tokio::net::unix::OwnedReadHalf,
    pending: PendingMap,
    events_tx: mpsc::UnboundedSender<CoreEvent>,
) {
    let mut reader = BufReader::new(read_half);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break, // EOF: core closed the connection
            Ok(_) => {}
            Err(_) => break,
        }
        if buf.len() > MAX_FRAME_BYTES {
            // Over-size inbound frame is a protocol error (§1.1); stop reading.
            break;
        }
        let trimmed = trim_newline(&buf);
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_slice(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // a malformed frame is skipped, not fatal
        };
        if value.get("t").and_then(Value::as_str) == Some("event") {
            if let Some(ev) = decode_event(&value) {
                if events_tx.send(ev).is_err() {
                    break; // no receiver left
                }
            }
            continue;
        }
        // A response frame, correlated by id.
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(decode_response(&value));
            }
        }
        // An `{id: null, error}` frame (frame-level error with no waiter) is dropped.
    }
    // On disconnect, fail every outstanding request rather than leaving it hung.
    let mut map = pending.lock().await;
    for (_, tx) in map.drain() {
        let _ = tx.send(Err(RpcError {
            code: "transport.closed".into(),
            message: "core connection closed".into(),
            retryable: true,
            data: None,
        }));
    }
}

/// Strip trailing `\n`/`\r` bytes from a raw NDJSON line.
fn trim_newline(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
        end -= 1;
    }
    &buf[..end]
}

/// Split a response frame into `Ok(ok-body)` or `Err(RpcError)`. A missing `ok`
/// body defaults to an empty object so callers can index it uniformly.
pub(super) fn decode_response(value: &Value) -> Result<Value, RpcError> {
    if let Some(err) = value.get("error") {
        return Err(RpcError {
            code: err
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            retryable: err
                .get("retryable")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            data: err.get("data").cloned(),
        });
    }
    Ok(value.get("ok").cloned().unwrap_or_else(|| json!({})))
}

/// Decode an event-stream envelope into a [`CoreEvent`]. Returns `None` when the
/// frame lacks the mandatory `seq` or `event` body.
pub(super) fn decode_event(value: &Value) -> Option<CoreEvent> {
    Some(CoreEvent {
        seq: value.get("seq").and_then(Value::as_u64)?,
        at: value.get("at").and_then(Value::as_i64).unwrap_or(0),
        thread_id: value
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        cycle_id: value
            .get("cycleId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        event: value.get("event").cloned()?,
    })
}
