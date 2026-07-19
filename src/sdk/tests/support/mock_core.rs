//! A configurable in-test mock of the core-js orchestration core over a Unix
//! socket (§1–§3 of the protocol contract). It answers the RPC methods
//! [`CoreRuntime`](medulla::runtime::core::CoreRuntime) issues, lets a test push
//! arbitrary event / raw frames, override any method's response with an error, and
//! drop the connection mid-flight to exercise the transport-closed paths.
//!
//! Included by test binaries via `#[path = "support/mock_core.rs"] mod mock_core;`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Tunable behaviour for the mock core.
#[derive(Default, Clone)]
pub struct MockCoreConfig {
    /// Per-method `ok` payload overrides (method → payload).
    pub responses: HashMap<String, Value>,
    /// Per-method error overrides (method → `{code,message,retryable,data}`). Wins
    /// over `responses` and the defaults.
    pub errors: HashMap<String, Value>,
    /// `initialize` response payload; defaults to a capable handshake.
    pub initialize: Option<Value>,
    /// When set, `thread.list` reports this one existing thread (so `connect` adopts
    /// it instead of creating a fresh one).
    pub existing_thread: Option<String>,
    /// Snapshot returned by `thread.subscribe`.
    pub subscribe_snapshot: Option<Value>,
    /// `baselineSeq` returned by `thread.subscribe`.
    pub baseline_seq: u64,
    /// When set, the connection is dropped (no response) the moment this method is
    /// received — exercising the transport-closed-mid-RPC path.
    pub close_on: Option<String>,
}

impl MockCoreConfig {
    pub fn with_error(mut self, method: &str, code: &str) -> Self {
        self.errors.insert(
            method.to_string(),
            json!({ "code": code, "message": code, "retryable": false }),
        );
        self
    }

    pub fn with_error_data(mut self, method: &str, code: &str, data: Value) -> Self {
        self.errors.insert(
            method.to_string(),
            json!({ "code": code, "message": code, "retryable": true, "data": data }),
        );
        self
    }
}

enum Out {
    Bytes(Vec<u8>),
    Close,
}

/// A running mock core. Drop it to stop the acceptor.
pub struct MockCore {
    pub path: PathBuf,
    out_tx: mpsc::UnboundedSender<Out>,
    frame_tx: mpsc::UnboundedSender<Value>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl MockCore {
    pub async fn start(path: &Path) -> MockCore {
        Self::start_with(path, MockCoreConfig::default()).await
    }

    pub async fn start_with(path: &Path, cfg: MockCoreConfig) -> MockCore {
        let listener = UnixListener::bind(path).unwrap();
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Value>();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Out>();
        let calls: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let workers: Arc<Mutex<(Vec<Value>, Option<String>)>> =
            Arc::new(Mutex::new((Vec::new(), None)));

        // Bridge event frames onto the raw-out channel.
        let out_for_frames = out_tx.clone();
        tokio::spawn(async move {
            while let Some(frame) = frame_rx.recv().await {
                let mut line = serde_json::to_vec(&frame).unwrap();
                line.push(b'\n');
                if out_for_frames.send(Out::Bytes(line)).is_err() {
                    break;
                }
            }
        });

        let calls_h = calls.clone();
        let workers_h = workers.clone();
        let out_for_reader = out_tx.clone();
        let cfg = Arc::new(cfg);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();

            // Writer: drain the raw-out channel to the socket.
            tokio::spawn(async move {
                while let Some(out) = out_rx.recv().await {
                    match out {
                        Out::Bytes(bytes) => {
                            if write_half.write_all(&bytes).await.is_err() {
                                break;
                            }
                            let _ = write_half.flush().await;
                        }
                        Out::Close => {
                            let _ = write_half.shutdown().await;
                            break;
                        }
                    }
                }
            });

            // Reader: handle each request, respond via the raw-out channel.
            let mut reader = BufReader::new(read_half);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buf);
                let req: Value = match serde_json::from_str(line.trim()) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let method = req
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let params = req.get("params").cloned().unwrap_or(json!({}));
                calls_h
                    .lock()
                    .unwrap()
                    .push((method.clone(), params.clone()));

                if cfg.close_on.as_deref() == Some(method.as_str()) {
                    let _ = out_for_reader.send(Out::Close);
                    break;
                }

                let response = match handle(&cfg, &method, &params, &workers_h) {
                    Ok(ok) => json!({ "id": id, "ok": ok }),
                    Err(err) => json!({ "id": id, "error": err }),
                };
                let mut resp_line = serde_json::to_vec(&response).unwrap();
                resp_line.push(b'\n');
                if out_for_reader.send(Out::Bytes(resp_line)).is_err() {
                    break;
                }
            }
        });

        MockCore {
            path: path.to_path_buf(),
            out_tx,
            frame_tx,
            calls,
        }
    }

    /// Push a `{t:"event", ...}` frame with the given body onto the connection.
    pub fn push_event(&self, seq: u64, cycle_id: &str, body: Value) {
        let frame = json!({
            "t": "event",
            "seq": seq,
            "at": now_ms(),
            "threadId": "th_test",
            "cycleId": cycle_id,
            "event": body,
        });
        let _ = self.frame_tx.send(frame);
    }

    /// Push an event addressed to a specific `threadId`.
    pub fn push_event_for(&self, thread_id: &str, seq: u64, cycle_id: &str, body: Value) {
        let frame = json!({
            "t": "event",
            "seq": seq,
            "at": now_ms(),
            "threadId": thread_id,
            "cycleId": cycle_id,
            "event": body,
        });
        let _ = self.frame_tx.send(frame);
    }

    /// Write a raw line (plus newline) directly — for malformed / non-JSON frames.
    pub fn push_raw_line(&self, line: &str) {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        let _ = self.out_tx.send(Out::Bytes(bytes));
    }

    /// Write an over-1-MiB frame, which the client treats as a protocol error
    /// (§1.1). The 1 MiB cap is [`medulla::runtime::core_client::MAX_FRAME_BYTES`].
    pub fn push_oversize_frame(&self) {
        let mut bytes = vec![b'x'; medulla::runtime::core_client::MAX_FRAME_BYTES + 16];
        bytes.push(b'\n');
        let _ = self.out_tx.send(Out::Bytes(bytes));
    }

    /// Drop the connection (server closes its write half → client sees EOF).
    pub fn close(&self) {
        let _ = self.out_tx.send(Out::Close);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }

    /// The params of the first recorded call to `method`, if any.
    pub fn params_of(&self, method: &str) -> Option<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
    }
}

/// Compute a response for one request method: `Ok(payload)` or `Err(error_obj)`.
fn handle(
    cfg: &MockCoreConfig,
    method: &str,
    params: &Value,
    workers: &Arc<Mutex<(Vec<Value>, Option<String>)>>,
) -> Result<Value, Value> {
    if let Some(err) = cfg.errors.get(method) {
        return Err(err.clone());
    }
    if let Some(ok) = cfg.responses.get(method) {
        return Ok(ok.clone());
    }
    let ok = match method {
        "initialize" => cfg.initialize.clone().unwrap_or_else(|| {
            json!({
                "coreVersion": "mock",
                "protocolVersion": "1",
                "capabilities": ["threads", "cycles", "events", "snapshot", "workers"],
            })
        }),
        "thread.list" => match &cfg.existing_thread {
            Some(id) => json!({ "threads": [{ "threadId": id, "name": "main", "cycleSeq": 2 }] }),
            None => json!({ "threads": [] }),
        },
        "thread.create" => json!({ "threadId": "th_test" }),
        "thread.resume" => json!({ "threadId": "th_test", "cycleSeq": 0 }),
        "thread.fork" => json!({ "threadId": "th_fork" }),
        "thread.subscribe" => {
            let mut m = serde_json::Map::new();
            m.insert("baselineSeq".into(), json!(cfg.baseline_seq));
            if let Some(snap) = &cfg.subscribe_snapshot {
                m.insert("snapshot".into(), snap.clone());
            }
            Value::Object(m)
        }
        "cycle.submit" => json!({ "cycleId": "cyc:app:th_test:1" }),
        "cycle.abort" | "task.cancel" | "question.answer" | "config.set" => json!({}),
        "context.inspect" => json!({
            "chunks": [
                { "id": "ctx-1", "kind": "note", "text": "remembered fact" }
            ],
        }),
        "roster.list" => json!({ "agents": [] }),
        "snapshot.get" => json!({
            "baselineSeq": params.get("sinceSeq").cloned().unwrap_or(json!(0)),
            "snapshot": {
                "at": now_ms(),
                "chat": [],
                "tasks": [{
                    "taskId": "t1",
                    "cycleId": "cyc:app:th_test:1",
                    "status": "running",
                    "instruction": "resynced",
                    "depth": 1
                }],
            },
        }),
        "worker.list" => {
            let g = workers.lock().unwrap();
            json!({ "workers": g.0.clone(), "selectedId": g.1.clone() })
        }
        "worker.add" => {
            let mut g = workers.lock().unwrap();
            let n = g.0.len() + 1;
            let id = format!("w_{n}");
            let address = params
                .get("address")
                .or_else(|| params.get("handle"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut w = serde_json::Map::new();
            w.insert("id".into(), json!(id));
            w.insert("address".into(), json!(address));
            for key in ["handle", "label", "harness"] {
                if let Some(v) = params.get(key).and_then(Value::as_str) {
                    w.insert(key.into(), json!(v));
                }
            }
            if g.1.is_none() {
                g.1 = Some(id.clone());
            }
            let selected = g.1.as_deref() == Some(id.as_str());
            w.insert("selected".into(), json!(selected));
            g.0.push(Value::Object(w.clone()));
            json!({ "worker": Value::Object(w), "selectedId": g.1.clone() })
        }
        "worker.update" => {
            let mut g = workers.lock().unwrap();
            let id = params.get("id").and_then(Value::as_str).unwrap_or("");
            if let Some(patch) = params.get("patch").and_then(Value::as_object) {
                for w in g.0.iter_mut() {
                    if w.get("id").and_then(Value::as_str) == Some(id) {
                        if let Some(obj) = w.as_object_mut() {
                            for (k, v) in patch {
                                obj.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
            }
            json!({ "workers": g.0.clone(), "selectedId": g.1.clone() })
        }
        "worker.select" => {
            let mut g = workers.lock().unwrap();
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            g.1 = Some(id.clone());
            for w in g.0.iter_mut() {
                let is = w.get("id").and_then(Value::as_str) == Some(id.as_str());
                if let Some(obj) = w.as_object_mut() {
                    obj.insert("selected".into(), json!(is));
                }
            }
            json!({ "workers": g.0.clone(), "selectedId": g.1.clone() })
        }
        "worker.remove" => {
            let mut g = workers.lock().unwrap();
            let id = params.get("id").and_then(Value::as_str).unwrap_or("");
            g.0.retain(|w| w.get("id").and_then(Value::as_str) != Some(id));
            json!({ "workers": g.0.clone(), "selectedId": g.1.clone() })
        }
        _ => json!({}),
    };
    Ok(ok)
}
