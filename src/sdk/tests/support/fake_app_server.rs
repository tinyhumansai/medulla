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
    /// File each process appends one line per request it sent *to* the client.
    ///
    /// Separate from [`Self::request_log`], which records the other direction.
    /// A test asserting that the client answered an elicitation needs the id the
    /// fake asked with, and that id is allocated at run time rather than written
    /// into the script, so it cannot be a constant in the test.
    pub ask_log: String,
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

    /// Every request the fake sent *to* the client, in send order.
    pub fn asks(&self) -> Vec<serde_json::Value> {
        read_lines(&self.ask_log)
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The id of the single request the fake asked the client.
    ///
    /// Panics when the fake asked anything other than exactly one, because a
    /// test correlating one answer against one question has nothing to say
    /// about which of several it meant.
    pub fn only_ask_id(&self) -> serde_json::Value {
        let asks = self.asks();
        assert_eq!(asks.len(), 1, "expected exactly one ask, got {asks:?}");
        asks[0]["id"].clone()
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
        ask("item/commandExecution/requestApproval", {"threadId": thread_id, "turnId": "turn-1", "itemId": "item-1", "startedAtMs": 0, "command": "rm -rf /"})
        notify("turn/completed", {"threadId": thread_id, "turn": {"id": "turn-1", "status": "completed", "items": []}})
        respond(message["id"], {"turn": {"id": "turn-1", "status": "completed", "items": []}})
"#
            .to_string(),
            TurnScript::Elicit => r#"
        ask("item/tool/requestUserInput", {"threadId": thread_id, "turnId": "turn-1"})
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
    let ask_log = dir.path().join("asks.log").to_string_lossy().into_owned();
    let body = format!(
        r#"#!/usr/bin/env python3
import itertools, json, os, sys, threading, uuid

SPAWN_LOG = {spawn_log:?}
REQUEST_LOG = {request_log:?}
ASK_LOG = {ask_log:?}

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

pending = {{}}
pending_lock = threading.Lock()
next_ask_id = itertools.count(9001)

def ask(method, params):
    """Send a request *to* the client and block until it answers.

    The id is allocated here rather than written into the script because turns
    are served concurrently: two overlapping turns asking under one hard-coded
    id would be indistinguishable on the wire, and both answers would release
    whichever wait registered last, leaving the other turn stuck until its
    timeout. The id the fake chose is recorded to ASK_LOG so a test can still
    correlate an answer against the question it answers.

    The answer arrives on stdin and is recorded like any other inbound line.
    Blocking here is what a real server does, and it is also what makes the
    recording observable: a script that asked and then completed the turn
    straight away would let the client's task finish before the reader loop had
    written the answer, so a test reading the request log would race it.
    """
    with pending_lock:
        request_id = next(next_ask_id)
        event = threading.Event()
        pending[request_id] = event
    payload = {{"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}}
    with open(ASK_LOG, "a") as handle:
        handle.write(json.dumps(payload) + "\n")
    write(payload)
    # Bounded, so a client that never answers hangs the turn rather than the
    # whole process — the test's own timeout then reports it.
    event.wait(30)
    with pending_lock:
        pending.pop(request_id, None)

def answered(message):
    """Release the `ask` waiting on this reply, if any."""
    with pending_lock:
        event = pending.get(message.get("id"))
    if event is not None:
        event.set()

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
    # It is recorded above, which is all a test needs; there is nothing to serve
    # beyond releasing the `ask` that is waiting on it.
    if "method" not in message:
        answered(message)
        continue
    if "id" not in message and message.get("method") != "turn/start":
        continue
    # Each turn is served on its own thread, so two concurrent lanes overlap the
    # way they would on a real server rather than queueing behind one another.
    threading.Thread(target=serve, args=(message,), daemon=True).start()
"#,
        spawn_log = spawn_log,
        request_log = request_log,
        ask_log = ask_log,
        turn = script.python(),
    );
    let bin = dir.write_script("codex-app-server-fake", &body);
    FakeAppServer {
        bin,
        spawn_log,
        request_log,
        ask_log,
    }
}
