//! An in-crate NDJSON stub of `medulla-serve` for the core-runtime unit tests,
//! mirroring the `tests/support` mock pattern but scoped to this module. It
//! speaks just enough of the protocol to exercise the handshake, an `instruct`
//! round trip, event streaming, version-mismatch / `hello` rejection, and
//! socket-drop reconnect. It is deterministic and offline: a unix listener on a
//! unique temp path, accepting repeatedly so a re-attach is served.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

/// How the stub should behave for the test in hand.
#[derive(Clone)]
pub(super) struct StubConfig {
    /// The `protocol` int advertised in the `ready` banner (2 ⇒ mismatch).
    pub(super) protocol: i64,
    /// Whether `hello` is answered `ok:true` (false ⇒ a fatal rejection).
    pub(super) hello_ok: bool,
    /// Close the connection right after an `instruct` (to force a reconnect).
    pub(super) drop_after_instruct: bool,
    /// Answer `instruct` with `ok:false` (to exercise the failed-request path).
    pub(super) instruct_fail: bool,
    /// The `event.event` payloads streamed after an `instruct` receipt.
    pub(super) instruct_events: Vec<Value>,
    /// The `event.event` payloads re-emitted when a `subscribe` carries a
    /// `replay` key (a re-attach rebaseline, serve-protocol §5). Empty ⇒ the
    /// stub replays nothing, matching a serve with no recent history.
    pub(super) replay_events: Vec<Value>,
    /// Raw frames to push right after answering `hello` (e.g. a `call` or a
    /// duplicate `ready`), exercising the host's inbound-frame handling.
    pub(super) after_hello: Vec<Value>,
}

impl Default for StubConfig {
    fn default() -> Self {
        StubConfig {
            protocol: 1,
            hello_ok: true,
            drop_after_instruct: false,
            instruct_fail: false,
            after_hello: Vec::new(),
            replay_events: Vec::new(),
            instruct_events: vec![
                json!({"kind":"cycle_start","instructionId":"inst-agent-0","cycleId":"cyc:agent:0"}),
                json!({"kind":"task_board_changed","task":{
                    "id":"t1","title":"reconcile","status":"active",
                    "createdAt":"0","updatedAt":"0","delegatedTaskIds":[],"notes":[]
                }}),
                json!({"kind":"cycle_end","instructionId":"inst-agent-0","cycleId":"cyc:agent:0"}),
            ],
        }
    }
}

/// A running stub. Drop aborts the accept loop and unlinks the socket.
pub(super) struct StubServer {
    /// The socket path to attach a [`CoreRuntime`](super::CoreRuntime) to.
    pub(super) path: PathBuf,
    /// How many connections have been accepted (grows across reconnects).
    accepts: Arc<AtomicUsize>,
    /// Every `(op, params)` received, in arrival order.
    received: Arc<Mutex<Vec<(String, Value)>>>,
    /// The accept-loop task.
    accept_task: JoinHandle<()>,
}

/// A process-unique temp socket path (kept short for the ~104-char sun_path cap).
fn unique_path() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("mdl-stub-{}-{n}.sock", std::process::id()))
}

impl StubServer {
    /// Bind and start serving under `cfg`.
    pub(super) fn start(cfg: StubConfig) -> StubServer {
        let path = unique_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind stub socket");
        let accepts = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let cfg = Arc::new(cfg);
        let (a, r) = (accepts.clone(), received.clone());
        let accept_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                a.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(handle_conn(stream, cfg.clone(), r.clone()));
            }
        });
        StubServer {
            path,
            accepts,
            received,
            accept_task,
        }
    }

    /// How many connections have been accepted so far.
    pub(super) fn accept_count(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }

    /// The ops received so far, in order.
    pub(super) fn received_ops(&self) -> Vec<String> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(op, _)| op.clone())
            .collect()
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.accept_task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Serve one connection: banner, then answer requests until the peer closes or
/// the config asks us to drop.
async fn handle_conn(
    stream: UnixStream,
    cfg: Arc<StubConfig>,
    received: Arc<Mutex<Vec<(String, Value)>>>,
) {
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);

    let ready = json!({
        "t":"ready","protocol":cfg.protocol,"serve":"3.12.0","sessionId":"agent",
        "capabilities":["inference","tools","subagents"],"error":null
    });
    if send(&mut wr, &ready).await.is_err() {
        return;
    }

    let mut seq: u64 = 0;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return, // peer closed
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue; // skip unparseable, like real serve
        };
        if value.get("t").and_then(Value::as_str) != Some("req") {
            continue;
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let op = value
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        received.lock().unwrap().push((op.clone(), params.clone()));

        let closed = serve_op(&mut wr, &cfg, &op, &id, &params, &mut seq).await;
        if closed {
            return;
        }
    }
}

/// Answer one request. Returns `true` when the connection should close.
async fn serve_op(
    wr: &mut tokio::net::unix::OwnedWriteHalf,
    cfg: &StubConfig,
    op: &str,
    id: &str,
    params: &Value,
    seq: &mut u64,
) -> bool {
    match op {
        "hello" => {
            let res = if cfg.hello_ok {
                json!({"t":"res","id":id,"ok":true,"result":{
                    "protocol":1,"sessionId":"agent","ports":["inference","tools","subagents"]
                }})
            } else {
                json!({"t":"res","id":id,"ok":false,
                    "error":{"code":"port_unavailable","message":"host missing a required port"}})
            };
            if send(wr, &res).await.is_err() {
                return true;
            }
            // Push any scripted post-hello frames (e.g. a reverse-RPC `call`).
            for frame in &cfg.after_hello {
                if send(wr, frame).await.is_err() {
                    return true;
                }
            }
            !cfg.hello_ok
        }
        "subscribe" => {
            let _ = send(
                wr,
                &json!({"t":"res","id":id,"ok":true,"result":{"subscribed":true,"seq":*seq}}),
            )
            .await;
            // A `replay` key means a re-attach rebaseline: re-emit recent events
            // so the host can rebuild its state (serve-protocol §5).
            if params.get("replay").is_some() {
                for event in &cfg.replay_events {
                    *seq += 1;
                    let frame = json!({"t":"event","seq":*seq,"at":0,"event":event});
                    if send(wr, &frame).await.is_err() {
                        return true;
                    }
                }
            }
            false
        }
        "instruct" if cfg.instruct_fail => {
            let _ = send(
                wr,
                &json!({"t":"res","id":id,"ok":false,
                "error":{"code":"internal","message":"harness rejected the instruction"}}),
            )
            .await;
            false
        }
        "instruct" => {
            let receipt = json!({"t":"res","id":id,"ok":true,"result":{
                "instructionId":"inst-agent-0","cycleId":"cyc:agent:0"
            }});
            if send(wr, &receipt).await.is_err() {
                return true;
            }
            for event in &cfg.instruct_events {
                *seq += 1;
                let frame = json!({"t":"event","seq":*seq,"at":0,"event":event});
                if send(wr, &frame).await.is_err() {
                    return true;
                }
            }
            cfg.drop_after_instruct
        }
        "answer_question" | "cancel_task" => {
            let _ = send(
                wr,
                &json!({"t":"res","id":id,"ok":true,"result":{"accepted":true}}),
            )
            .await;
            false
        }
        "stop" => {
            let _ = send(
                wr,
                &json!({"t":"res","id":id,"ok":true,"result":{"stopped":true}}),
            )
            .await;
            false
        }
        _ => {
            let _ = send(
                wr,
                &json!({"t":"res","id":id,"ok":false,
                "error":{"code":"unknown_op","message":"unknown op"}}),
            )
            .await;
            false
        }
    }
}

/// Write one NDJSON frame (newline-terminated).
async fn send(wr: &mut tokio::net::unix::OwnedWriteHalf, frame: &Value) -> std::io::Result<()> {
    wr.write_all(format!("{frame}\n").as_bytes()).await
}
