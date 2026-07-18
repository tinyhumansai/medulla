//! `CoreClient` — an async NDJSON RPC client over the core-js Unix socket
//! (§1–§3 of docs/unified-app/04-protocol-contract.md).
//!
//! It owns one `tokio::net::UnixStream`, split into a write half (behind a mutex,
//! used by `request`) and a read half driven by a background task. That task:
//!
//!   - correlates each `{id, ok|error}` response with the `request` that is awaiting
//!     it (via a per-id `oneshot`), and
//!   - forwards every unsolicited `{"t":"event", ...}` frame to the events channel
//!     handed back from [`connect`], so the runtime can fold the stream (§3.2).
//!
//! Frames are newline-delimited JSON with a 1 MiB cap (§1.1); an over-size frame is a
//! protocol error, never a truncation. `serde_json` handles (de)serialization — no
//! hand-rolled JSON, unlike the std-only scaffold this replaces.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};

/// The wire protocol version this client speaks (§2). Must equal core-js's
/// `MEDULLA_PROTOCOL_VERSION`.
pub const PROTOCOL_VERSION: &str = "1";

/// 1 MiB frame cap (§1.1). An over-size frame is a protocol error.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One decoded event-stream frame (§3.2): the envelope plus the raw `event` body.
#[derive(Debug, Clone)]
pub struct CoreEvent {
    pub seq: u64,
    pub at: i64,
    pub thread_id: String,
    pub cycle_id: String,
    /// The event body `{kind, ...}` — deserialized into a `TuiEvent` by the runtime.
    pub event: Value,
}

impl CoreEvent {
    pub fn kind(&self) -> &str {
        self.event.get("kind").and_then(Value::as_str).unwrap_or("")
    }
}

/// An RPC error body (§5.1/§5.2). `data` carries structured detail — notably the
/// `{baselineSeq, snapshot}` a `resync.required` hands back so a client can rebaseline
/// without a second round-trip.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} (retryable={})",
            self.code, self.message, self.retryable
        )
    }
}

/// A failed `request`: either the transport broke or the core returned an RPC error.
#[derive(Debug)]
pub enum CallError {
    Transport(String),
    Rpc(RpcError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Transport(m) => write!(f, "transport: {m}"),
            CallError::Rpc(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CallError {}

impl CallError {
    /// The RPC error code, when this is an RPC error (for `resync.required` etc.).
    pub fn rpc_code(&self) -> Option<&str> {
        match self {
            CallError::Rpc(e) => Some(&e.code),
            _ => None,
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// A connected core client.
pub struct CoreClient {
    socket_path: std::path::PathBuf,
    write: Arc<Mutex<OwnedWriteHalf>>,
    next_id: AtomicU64,
    pending: PendingMap,
}

/// Resolve the core socket path (§1.1). An explicit `override_path` (the `--core`
/// flag or `[core].socketPath` config) wins; otherwise `$XDG_RUNTIME_DIR/medulla/
/// core.sock`, then `<state_dir>/core.sock`. `None` when nothing is available.
pub fn resolve_socket_path(
    override_path: Option<&str>,
    runtime_dir: Option<&str>,
    state_dir: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(dir) = runtime_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("medulla").join("core.sock"));
    }
    if let Some(dir) = state_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("core.sock"));
    }
    None
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

    /// The `initialize` handshake / liveness probe (§2).
    pub async fn initialize(&self, client_version: &str) -> Result<Value, CallError> {
        self.request(
            "initialize",
            json!({ "clientVersion": client_version, "protocolVersion": PROTOCOL_VERSION }),
        )
        .await
    }

    pub async fn thread_list(&self) -> Result<Value, CallError> {
        self.request("thread.list", json!({})).await
    }

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

    pub async fn thread_resume(&self, thread_id: &str) -> Result<Value, CallError> {
        self.request("thread.resume", json!({ "threadId": thread_id }))
            .await
    }

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
    /// resync?, snapshot?}` (§3.2). Events then arrive on the [`connect`] receiver.
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

    pub async fn cycle_abort(&self, cycle_id: &str) -> Result<Value, CallError> {
        self.request("cycle.abort", json!({ "cycleId": cycle_id }))
            .await
    }

    pub async fn task_cancel(&self, cycle_id: &str, task_id: &str) -> Result<Value, CallError> {
        self.request(
            "task.cancel",
            json!({ "cycleId": cycle_id, "taskId": task_id }),
        )
        .await
    }

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

    pub async fn roster_list(&self) -> Result<Value, CallError> {
        self.request("roster.list", json!({})).await
    }

    pub async fn context_inspect(&self, cycle_id: &str) -> Result<Value, CallError> {
        self.request("context.inspect", json!({ "cycleId": cycle_id }))
            .await
    }

    pub async fn config_set(&self, thread_id: &str, patch: Value) -> Result<Value, CallError> {
        self.request(
            "config.set",
            json!({ "threadId": thread_id, "patch": patch }),
        )
        .await
    }

    // --- worker.* (managed remote peers) ----------------------------------------

    pub async fn worker_list(&self) -> Result<Value, CallError> {
        self.request("worker.list", json!({})).await
    }

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

    pub async fn worker_update(&self, id: &str, patch: Value) -> Result<Value, CallError> {
        self.request("worker.update", json!({ "id": id, "patch": patch }))
            .await
    }

    pub async fn worker_remove(&self, id: &str) -> Result<Value, CallError> {
        self.request("worker.remove", json!({ "id": id })).await
    }

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

fn trim_newline(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
        end -= 1;
    }
    &buf[..end]
}

fn decode_response(value: &Value) -> Result<Value, RpcError> {
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

fn decode_event(value: &Value) -> Option<CoreEvent> {
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

/// Tracks a thread's event `seq` to detect gaps (§3.2). A gap means the core
/// coalesced/dropped frames and the client should `snapshot.get` to resync.
#[derive(Debug, Clone)]
pub struct SeqTracker {
    last_seq: u64,
}

impl SeqTracker {
    /// Start from a subscribe `baselineSeq`.
    pub fn new(baseline_seq: u64) -> Self {
        SeqTracker {
            last_seq: baseline_seq,
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Record an event's seq. Returns `true` when a gap was detected (this seq is
    /// beyond the next expected). Advances regardless, so the client resyncs from here.
    pub fn observe(&mut self, seq: u64) -> bool {
        let expected = self.last_seq + 1;
        let gap = seq > expected;
        if seq > self.last_seq {
            self.last_seq = seq;
        }
        gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_override_then_xdg_then_state() {
        let over = resolve_socket_path(Some("/tmp/x.sock"), Some("/run/user/1000"), Some("/state"));
        assert_eq!(over.unwrap(), PathBuf::from("/tmp/x.sock"));

        let xdg = resolve_socket_path(None, Some("/run/user/1000"), Some("/state"));
        assert_eq!(
            xdg.unwrap(),
            PathBuf::from("/run/user/1000/medulla/core.sock")
        );

        let state = resolve_socket_path(None, None, Some("/state"));
        assert_eq!(state.unwrap(), PathBuf::from("/state/core.sock"));

        assert!(resolve_socket_path(None, None, None).is_none());
        // Empty strings are treated as unset.
        assert!(resolve_socket_path(Some(""), None, None).is_none());
    }

    #[test]
    fn seq_tracker_detects_a_gap() {
        let mut t = SeqTracker::new(0);
        assert!(!t.observe(1));
        assert!(!t.observe(2));
        assert!(t.observe(10)); // core coalesced 3..9
        assert_eq!(t.last_seq(), 10);
        assert!(!t.observe(11));
    }

    #[test]
    fn decode_event_reads_the_envelope() {
        let frame = json!({
            "t": "event", "seq": 5, "at": 42, "threadId": "th_x",
            "cycleId": "cyc:app:th_x:1", "event": {"kind": "assistant", "body": "hi"}
        });
        let ev = decode_event(&frame).unwrap();
        assert_eq!(ev.seq, 5);
        assert_eq!(ev.at, 42);
        assert_eq!(ev.kind(), "assistant");
        assert_eq!(ev.cycle_id, "cyc:app:th_x:1");
    }

    #[test]
    fn decode_response_splits_ok_and_error() {
        let ok = json!({ "id": 1, "ok": { "threadId": "th_x" } });
        assert_eq!(
            decode_response(&ok)
                .unwrap()
                .get("threadId")
                .and_then(Value::as_str),
            Some("th_x")
        );
        let err = json!({ "id": 2, "error": { "code": "thread.not-found", "message": "no", "retryable": false } });
        let e = decode_response(&err).unwrap_err();
        assert_eq!(e.code, "thread.not-found");
        assert!(!e.retryable);
    }
}
