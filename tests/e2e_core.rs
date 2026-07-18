//! End-to-end tests for the core-js runtime path: a mock core server on a tempdir
//! Unix socket scripts the NDJSON RPC + event wire (§1–§3 of the protocol contract),
//! and drives [`CoreRuntime`] through a real cycle, a seq-gap resync, and a worker
//! round trip.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use medulla::agents::{derive_agent_lanes, TaskStatus};
use medulla::core_client::CoreClient;
use medulla::core_runtime::CoreRuntime;
use medulla::runtime::{Runtime, WorkerOp};

use support::wait_until;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// A scripted mock core: answers the RPC methods `CoreRuntime` issues and lets the
/// test push arbitrary event frames onto the connection.
struct MockCore {
    #[allow(dead_code)]
    path: PathBuf,
    /// Push a raw event frame `{t:"event", ...}` onto the connection.
    frame_tx: mpsc::UnboundedSender<Value>,
    /// Every request method the server has received, in order.
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockCore {
    async fn start(path: &Path) -> MockCore {
        let listener = UnixListener::bind(path).unwrap();
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Value>();
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let workers: Arc<Mutex<(Vec<Value>, Option<String>)>> = Arc::new(Mutex::new((Vec::new(), None)));

        let calls_h = calls.clone();
        let workers_h = workers.clone();
        let out_tx = frame_tx.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();

            // Writer: drain the frame channel to the socket.
            tokio::spawn(async move {
                while let Some(frame) = frame_rx.recv().await {
                    let mut line = serde_json::to_vec(&frame).unwrap();
                    line.push(b'\n');
                    if write_half.write_all(&line).await.is_err() {
                        break;
                    }
                    let _ = write_half.flush().await;
                }
            });

            // Reader: handle each request, respond via the writer channel.
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
                let method = req.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let params = req.get("params").cloned().unwrap_or(json!({}));
                calls_h.lock().unwrap().push(method.clone());
                let ok = handle(&method, &params, &workers_h);
                let _ = out_tx.send(json!({ "id": id, "ok": ok }));
            }
        });

        MockCore { path: path.to_path_buf(), frame_tx, calls }
    }

    fn push_event(&self, seq: u64, cycle_id: &str, body: Value) {
        let frame = json!({
            "t": "event",
            "seq": seq,
            "at": now_ms(),
            "threadId": "th_test",
            "cycleId": cycle_id,
            "event": body,
        });
        self.frame_tx.send(frame).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

/// Compute a response `ok` payload for one request method.
fn handle(method: &str, params: &Value, workers: &Arc<Mutex<(Vec<Value>, Option<String>)>>) -> Value {
    match method {
        "initialize" => json!({
            "coreVersion": "mock",
            "protocolVersion": "1",
            "capabilities": ["threads", "cycles", "events", "snapshot", "workers"],
        }),
        "thread.list" => json!({ "threads": [] }),
        "thread.create" => json!({ "threadId": "th_test" }),
        "thread.resume" => json!({ "threadId": "th_test", "cycleSeq": 0 }),
        "thread.fork" => json!({ "threadId": "th_fork" }),
        "thread.subscribe" => json!({ "baselineSeq": 0 }),
        "cycle.submit" => json!({ "cycleId": "cyc:app:th_test:1" }),
        "cycle.abort" | "task.cancel" | "question.answer" => json!({}),
        "context.inspect" => json!({ "chunks": [] }),
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
            if let Some(h) = params.get("handle").and_then(Value::as_str) {
                w.insert("handle".into(), json!(h));
            }
            if let Some(l) = params.get("label").and_then(Value::as_str) {
                w.insert("label".into(), json!(l));
            }
            if let Some(h) = params.get("harness").and_then(Value::as_str) {
                w.insert("harness".into(), json!(h));
            }
            if g.1.is_none() {
                g.1 = Some(id.clone());
            }
            let selected = g.1.as_deref() == Some(id.as_str());
            w.insert("selected".into(), json!(selected));
            g.0.push(Value::Object(w.clone()));
            json!({ "worker": Value::Object(w), "selectedId": g.1.clone() })
        }
        "worker.select" => {
            let mut g = workers.lock().unwrap();
            let id = params.get("id").and_then(Value::as_str).unwrap_or("").to_string();
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
            let id = params.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            g.0.retain(|w| w.get("id").and_then(Value::as_str) != Some(id.as_str()));
            json!({ "workers": g.0.clone(), "selectedId": g.1.clone() })
        }
        _ => json!({}),
    }
}

async fn connect(sock: &Path) -> CoreRuntime {
    let (client, rx) = CoreClient::connect(sock).await.unwrap();
    CoreRuntime::connect(client, rx, "test").await.unwrap()
}

#[tokio::test]
async fn core_cycle_folds_lanes_and_running_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    // Submit → optimistic running, cycle.submit issued.
    rt.submit("hi".into()).await.unwrap();
    assert!(rt.snapshot().running, "running is optimistic after submit");

    let cyc = "cyc:app:th_test:1";
    mock.push_event(1, cyc, json!({"kind":"cycle_start","cycleId":cyc}));
    mock.push_event(2, cyc, json!({"kind":"task_start","taskId":"t1","instruction":"do it","depth":1}));
    mock.push_event(3, cyc, json!({"kind":"task_event","taskId":"t1","eventKind":"text","content":"working","harness":"echo"}));
    mock.push_event(4, cyc, json!({"kind":"task_attention","taskId":"t1","reason":"approval","content":"ok?","questionId":"q_1"}));
    mock.push_event(5, cyc, json!({"kind":"task_complete","taskId":"t1","status":"cancelled","digest":"stopped"}));
    mock.push_event(6, cyc, json!({"kind":"cycle_end","cycleId":cyc,"passCount":1,"durationMs":10}));

    wait_until("cycle_end folds and clears running", Duration::from_secs(5), || {
        !rt.snapshot().running
    })
    .await;

    let snap = rt.snapshot();
    assert!(!snap.running, "cycle_end clears running");
    let lanes = derive_agent_lanes(&snap.events, "CORE", &[]);
    // The lane is keyed by the composite (cycleId, taskId) — §3.3(2).
    let lane = lanes
        .iter()
        .find(|l| l.key.contains("/t:t1"))
        .expect("a cycle-scoped worker lane");
    assert_eq!(lane.tasks.len(), 1);
    // §3.3(3): cancelled stays distinct from failed.
    assert_eq!(lane.tasks[0].status, TaskStatus::Cancelled);
    // The pending question was captured for steering, then cleared on completion.
    assert!(lane.tasks[0].question_id.is_none());
}

#[tokio::test]
async fn core_seq_gap_triggers_snapshot_resync() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;
    rt.submit("go".into()).await.unwrap();

    let cyc = "cyc:app:th_test:1";
    mock.push_event(1, cyc, json!({"kind":"cycle_start","cycleId":cyc}));
    mock.push_event(2, cyc, json!({"kind":"task_start","taskId":"t1","instruction":"a","depth":1}));
    // Gap: the next seq jumps to 10 (the core coalesced 3..9) — forces a resync.
    mock.push_event(10, cyc, json!({"kind":"task_event","taskId":"t1","eventKind":"text","content":"late"}));

    wait_until("snapshot.get is called on the seq gap", Duration::from_secs(5), || {
        mock.calls().iter().any(|m| m == "snapshot.get")
    })
    .await;

    // The rebuild seeded a lane from the returned snapshot.
    wait_until("resync rebuilds a lane", Duration::from_secs(5), || {
        let lanes = derive_agent_lanes(&rt.snapshot().events, "CORE", &[]);
        lanes.iter().any(|l| l.key.contains("/t:t1"))
    })
    .await;
}

#[tokio::test]
async fn core_worker_registry_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    assert!(rt.workers().is_empty(), "registry starts empty");

    rt.worker_op(WorkerOp::Add {
        address: Some("addr1".into()),
        handle: None,
        label: Some("dev".into()),
        harness: Some("claude".into()),
    })
    .await
    .unwrap();

    let ws = rt.workers();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].address, "addr1");
    assert_eq!(ws[0].label.as_deref(), Some("dev"));
    assert_eq!(ws[0].harness.as_deref(), Some("claude"));
    assert!(ws[0].selected, "the first worker is selected by default");

    let id = ws[0].id.clone();
    rt.worker_op(WorkerOp::Select { id: id.clone() }).await.unwrap();
    assert!(rt.workers().iter().any(|w| w.id == id && w.selected));

    rt.worker_op(WorkerOp::Remove { id }).await.unwrap();
    assert!(rt.workers().is_empty(), "removal clears the registry");
}
