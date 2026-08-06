//! A scripted stand-in for `codex app-server`, for exercising the pooled
//! transport without a real Codex or a network.
//!
//! Writes a small Python program that speaks the same line-framed JSON-RPC the
//! real server does: it answers `initialize`, `thread/start` and `thread/resume`
//! with a thread, and on `turn/start` replays a canned notification script
//! before answering the request.
//!
//! Python rather than a shell script because the server has to *read* JSON from
//! stdin and correlate ids, which is exactly what shell is worst at — and the
//! correlation is the behaviour under test.
//!
//! The script also records every request it received to a file, so a test can
//! assert what crossed the wire — how many processes were spawned, and whether
//! two lanes shared one.

use std::path::Path;

use super::fake_provider::TempDir;

/// A fake server plus the paths a test needs to drive and inspect it.
pub struct FakeAppServer {
    /// The `codex` stand-in to point `TINYPLACE_CODEX_BIN` at.
    pub bin: String,
    /// File each spawned process appends one line to, for counting processes.
    pub spawn_log: String,
    /// File each process appends one JSON request line to.
    pub request_log: String,
}

impl FakeAppServer {
    /// How many processes were spawned against this fake.
    ///
    /// The point of the transport is that this stays at one however many tasks
    /// run, so it is the assertion most of these tests are really making.
    pub fn spawn_count(&self) -> usize {
        read_lines(&self.spawn_log).len()
    }

    /// Every request the fake received, in arrival order, as decoded JSON.
    pub fn requests(&self) -> Vec<serde_json::Value> {
        read_lines(&self.request_log)
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The methods requested, in order.
    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter_map(|request| {
                request
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }
}

/// Non-empty lines of a file that may not exist yet.
fn read_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(Path::new(path))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// What the fake does when a turn starts.
pub enum TurnScript {
    /// Emit a couple of items and complete with `reply` as the assistant text.
    Reply(&'static str),
    /// Complete the turn with `status: failed`, preceded by an error
    /// notification carrying `message`.
    Fail(&'static str),
    /// Emit nothing at all and never complete, so the idle watchdog fires.
    Hang,
    /// Ask the client to approve a command, then complete the turn.
    ///
    /// The decision the client sends back is written to the request log like any
    /// other inbound line, so a test can assert what was answered.
    AskApproval,
    /// Ask the client for something it cannot answer honestly, then complete.
    Elicit,
    /// Die mid-turn, without answering, as a crashed server would.
    Die,
}

impl TurnScript {
    /// The Python branch body implementing this script.
    fn python(&self) -> String {
        match self {
            TurnScript::Reply(text) => format!(
                r#"
        notify("turn/started", {{"threadId": thread_id, "turn": {{"id": "turn-1", "status": "inProgress", "items": []}}}})
        notify("item/started", {{"threadId": thread_id, "turnId": "turn-1", "item": {{"type": "commandExecution", "command": "echo hi"}}}})
        notify("thread/tokenUsage/updated", {{"threadId": thread_id, "turnId": "turn-1", "tokenUsage": {{"total": {{"inputTokens": 11, "outputTokens": 7}}}}}})
        notify("item/completed", {{"threadId": thread_id, "turnId": "turn-1", "item": {{"type": "agentMessage", "text": {text:?}}}}})
        notify("turn/completed", {{"threadId": thread_id, "turn": {{"id": "turn-1", "status": "completed", "items": []}}}})
        respond(message["id"], {{"turn": {{"id": "turn-1", "status": "completed", "items": []}}}})
"#
            ),
            TurnScript::Fail(message) => format!(
                r#"
        notify("turn/started", {{"threadId": thread_id, "turn": {{"id": "turn-1", "status": "inProgress", "items": []}}}})
        notify("error", {{"threadId": thread_id, "turnId": "turn-1", "willRetry": False, "error": {{"message": {message:?}}}}})
        notify("turn/completed", {{"threadId": thread_id, "turn": {{"id": "turn-1", "status": "failed", "items": []}}}})
        respond(message["id"], {{"turn": {{"id": "turn-1", "status": "failed", "items": []}}}})
"#
            ),
            TurnScript::Hang => r#"
        pass
"#
            .to_string(),
            TurnScript::AskApproval => r#"
        ask(9001, "item/commandExecution/requestApproval", {"threadId": thread_id, "turnId": "turn-1", "itemId": "item-1", "startedAtMs": 0, "command": "rm -rf /"})
        notify("turn/completed", {"threadId": thread_id, "turn": {"id": "turn-1", "status": "completed", "items": []}})
        respond(message["id"], {"turn": {"id": "turn-1", "status": "completed", "items": []}})
"#
            .to_string(),
            TurnScript::Elicit => r#"
        ask(9002, "item/tool/requestUserInput", {"threadId": thread_id, "turnId": "turn-1"})
        notify("turn/completed", {"threadId": thread_id, "turn": {"id": "turn-1", "status": "completed", "items": []}})
        respond(message["id"], {"turn": {"id": "turn-1", "status": "completed", "items": []}})
"#
            .to_string(),
            TurnScript::Die => r#"
        notify("turn/started", {"threadId": thread_id, "turn": {"id": "turn-1", "status": "inProgress", "items": []}})
        os._exit(1)
"#
            .to_string(),
        }
    }
}

/// Write a fake app-server into `dir` that answers turns with `script`.
pub fn fake_app_server(dir: &TempDir, script: TurnScript) -> FakeAppServer {
    let spawn_log = dir.path().join("spawns.log").to_string_lossy().into_owned();
    let request_log = dir
        .path()
        .join("requests.log")
        .to_string_lossy()
        .into_owned();
    let body = format!(
        r#"#!/usr/bin/env python3
import json, os, sys, threading, uuid

SPAWN_LOG = {spawn_log:?}
REQUEST_LOG = {request_log:?}

# `codex app-server` is a subcommand of the same binary; anything else is a
# different invocation and must not count as a server start.
if "app-server" not in sys.argv[1:]:
    sys.exit(0)

with open(SPAWN_LOG, "a") as handle:
    handle.write("spawn\n")

lock = threading.Lock()

def write(payload):
    with lock:
        sys.stdout.write(json.dumps(payload) + "\n")
        sys.stdout.flush()

def notify(method, params):
    write({{"jsonrpc": "2.0", "method": method, "params": params}})

def respond(request_id, result):
    write({{"jsonrpc": "2.0", "id": request_id, "result": result}})

def fail(request_id, text):
    write({{"jsonrpc": "2.0", "id": request_id, "error": {{"code": -32000, "message": text}}}})

def ask(request_id, method, params):
    """Send a request *to* the client. Its answer arrives on stdin and is
    recorded like any other inbound line."""
    write({{"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}})

def serve(message):
    method = message.get("method")
    params = message.get("params") or {{}}
    if method == "initialize":
        respond(message["id"], {{"userAgent": "fake/0"}})
    elif method == "thread/start":
        thread_id = "thread-" + uuid.uuid4().hex[:8]
        respond(message["id"], {{"thread": {{"id": thread_id}}}})
        notify("thread/started", {{"thread": {{"id": thread_id}}}})
    elif method == "thread/resume":
        # Resuming an id this process never minted is refused, exactly as a real
        # server refuses a thread rolled off its history.
        if not str(params.get("threadId", "")).startswith("thread-"):
            fail(message["id"], "no such thread")
        else:
            respond(message["id"], {{"thread": {{"id": params["threadId"]}}}})
    elif method == "turn/start":
        thread_id = params["threadId"]
{turn}
    elif method == "turn/interrupt":
        respond(message["id"], {{}})
    elif "id" in message:
        fail(message["id"], "unsupported: " + str(method))

# `readline` rather than iterating `sys.stdin`: iteration reads ahead into an
# internal buffer, which stalls a client that is waiting for the answer to the
# line it just wrote.
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.strip()
    if not line:
        continue
    with open(REQUEST_LOG, "a") as handle:
        handle.write(line + "\n")
    message = json.loads(line)
    # A line with no method is the client answering something this fake asked.
    # It is recorded above, which is all a test needs; there is nothing to serve.
    if "method" not in message:
        continue
    if "id" not in message and message.get("method") != "turn/start":
        continue
    # Each turn is served on its own thread, so two concurrent lanes overlap the
    # way they would on a real server rather than queueing behind one another.
    threading.Thread(target=serve, args=(message,), daemon=True).start()
"#,
        spawn_log = spawn_log,
        request_log = request_log,
        turn = script.python(),
    );
    let bin = dir.write_script("codex-app-server-fake", &body);
    FakeAppServer {
        bin,
        spawn_log,
        request_log,
    }
}
