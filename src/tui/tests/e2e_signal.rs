//! End-to-end Signal transport suite: the REAL vendored `tinyplace` SDK (live
//! X3DH + double-ratchet crypto) talking to a MOCK tiny.place Signal server
//! ([`mock_signal_server`]). Only the transport server is mocked — every byte of
//! encryption runs in the SDK on both ends.
//!
//! See `tests/support/mock_signal_server.rs` for the endpoint spec, state model,
//! fault-injection controls, and the scenario matrix these tests realize.

#[path = "../../sdk/tests/support/mod.rs"]
mod support;

#[path = "../../sdk/tests/support/mock_harness.rs"]
mod mock_harness;
#[path = "../../sdk/tests/support/mock_signal_server.rs"]
mod mock_signal_server;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::providers::{run_provider_task, RunTaskFn, RunTaskOptions};
use medulla::daemon::transport::SignalTransport;
use medulla::daemon::{DaemonConfig, DaemonRuntime, SendFn};
use medulla::tinyplace_support::tinyplace::{
    LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions,
};
use medulla::tinyplace_support::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, HarnessProvider, TaskFrame,
    TaskFrameKind, TINYPLACE_PROTO,
};

use mock_harness::{MockCli, MockDir, MockProvider};
use mock_signal_server::MockSignalServer;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp dir removed on drop, hosting one identity's on-disk Signal state.
struct IdentityDir {
    path: PathBuf,
}

impl IdentityDir {
    fn new(tag: &str) -> Self {
        let id = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "medulla-e2e-signal-{tag}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        IdentityDir { path }
    }
}

impl Drop for IdentityDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A fully wired identity: signer, a server-pointed client, its transport, and
/// the temp dir keeping its on-disk Signal state alive.
struct Identity {
    transport: SignalTransport,
    signer: Arc<LocalSigner>,
    _dir: IdentityDir,
}

impl Identity {
    fn id(&self) -> String {
        self.signer.agent_id()
    }
}

/// Build an identity (fresh wallet) pointed at `base_url`.
fn make_identity(tag: &str, base_url: &str) -> Identity {
    let signer = Arc::new(LocalSigner::generate());
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.to_string(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let dir = IdentityDir::new(tag);
    let transport = SignalTransport::new(client, &signer, &dir.path);
    Identity {
        transport,
        signer,
        _dir: dir,
    }
}

/// Poll `predicate` until true or `timeout` elapses; returns the final value.
async fn poll_until<F>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const T: Duration = Duration::from_secs(10);

#[tokio::test]
async fn mock_signal_server_boots_and_serves_loopback() {
    let server = MockSignalServer::start().await;
    assert!(server.base_url.starts_with("http://127.0.0.1:"));
    // A fresh server has an empty stored log and zero counters.
    assert_eq!(server.controls().queued_messages(), 0);
    assert_eq!(server.controls().sends(), 0);
    assert!(server.controls().stored_envelopes().is_empty());
}

// Scenario 1: two identities (owner + worker) register (publish bundles),
// exchange bundles, and round-trip encrypted DMs through the real SDK. The
// server-side payloads are asserted ciphertext (no plaintext leakage) and each
// decryption yields the exact sent frame.
#[tokio::test]
async fn two_identities_register_and_roundtrip_encrypted_dms() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner", &server.base_url);
    let worker = make_identity("worker", &server.base_url);

    // Registration: both publish signed + one-time pre-keys so either can open a
    // channel to the other.
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    let owner_id = owner.id();
    let worker_id = worker.id();

    const TO_WORKER: &str = "PLAINTEXT-owner-to-worker-marker";
    const TO_OWNER: &str = "PLAINTEXT-worker-to-owner-marker";

    // owner → worker (opens a session from worker's bundle: X3DH + first ratchet).
    owner.transport.send(&worker_id, TO_WORKER).await.unwrap();
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from, owner_id);
    assert_eq!(inbox[0].text, TO_WORKER, "decryption yields the sent frame");

    // worker → owner reply (worker now opens its own session to owner).
    worker.transport.send(&owner_id, TO_OWNER).await.unwrap();
    let inbox = owner.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, TO_OWNER);

    // A second owner → worker send reuses the established session (no new bundle).
    let fetches_before = server.controls().bundle_fetches();
    owner
        .transport
        .send(&worker_id, "PLAINTEXT-again-marker")
        .await
        .unwrap();
    assert_eq!(
        server.controls().bundle_fetches(),
        fetches_before,
        "an established session does not re-fetch a bundle"
    );
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "PLAINTEXT-again-marker");

    // The server only ever saw ciphertext: none of the plaintext markers leaked
    // into any stored envelope (raw JSON or base64-decoded body bytes).
    server.assert_ciphertext_only(&[
        TO_WORKER,
        TO_OWNER,
        "PLAINTEXT-again-marker",
        "owner-to-worker",
        "worker-to-owner",
    ]);

    // Acks drained every delivered envelope from the live queue.
    assert_eq!(server.controls().queued_messages(), 0);
    assert!(server.controls().acks() >= 3);
}

// Scenario 1 (cont.): the presence + contacts REST surface the runtime loops
// drive is served faithfully — heartbeat, batch presence query, and the
// contact-accept handshake — against the same mock Signal server.
#[tokio::test]
async fn presence_and_contacts_rest_surface() {
    use medulla::tinyplace_support::{spawn_contact_auto_accepter, spawn_presence_heartbeat};

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-rest", &server.base_url);
    let worker = make_identity("worker-rest", &server.base_url);
    let owner_id = owner.id();
    let worker_id = worker.id();

    // The worker reports the owner online.
    server
        .controls()
        .set_online(std::slice::from_ref(&owner_id));
    // A pending contact request from the owner, which the worker auto-accepts.
    server.controls().add_pending_contact(&owner_id);

    // Rebuild a bare client for the worker (the transport does not expose one).
    let worker_client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: server.base_url.clone(),
        signer: Some(worker.signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });

    let heartbeat = spawn_presence_heartbeat(worker_client.clone(), Duration::from_millis(50));
    let allowed = owner_id.clone();
    let accepter = spawn_contact_auto_accepter(
        worker_client.clone(),
        Duration::from_millis(50),
        move |id: &str| id == allowed,
    );

    // Heartbeats flow and the contact is accepted.
    assert!(
        poll_until(T, || server.controls().heartbeats() >= 1).await,
        "presence heartbeat never reached the server"
    );
    assert!(
        poll_until(T, || server.controls().accepted().contains(&owner_id)).await,
        "contact auto-accept never reached the server"
    );

    heartbeat.abort();
    accepter.abort();

    // A direct presence query resolves the seeded online/offline split.
    let response = worker_client
        .presence
        .query(&[owner_id.clone(), worker_id.clone()])
        .await
        .unwrap();
    let online: std::collections::HashMap<String, bool> = response
        .presence
        .into_iter()
        .map(|p| (p.crypto_id, p.online))
        .collect();
    assert_eq!(online.get(&owner_id), Some(&true));
    assert_eq!(online.get(&worker_id), Some(&false));
    assert!(server.controls().presence_queries() >= 1);
}

// ─── daemon task-chain harness ──────────────────────────────────────────────

/// A transport-backed [`SendFn`] that encrypts every daemon reply to its peer
/// over the real Signal transport.
fn transport_send(transport: SignalTransport) -> SendFn {
    Arc::new(move |to: String, body: String| {
        let transport = transport.clone();
        Box::pin(async move {
            if let Err(err) = transport.send(&to, &body).await {
                eprintln!("mock daemon send failed: {err}");
            }
        })
    })
}

fn daemon_config(
    provider: HarnessProvider,
    workspace: String,
    env: HashMap<String, String>,
) -> DaemonConfig {
    DaemonConfig {
        providers: vec![provider],
        default_provider: provider,
        workspace,
        env,
        task_timeout_ms: 10_000,
        capability_timeout_ms: Some(10_000),
        concurrency: 2,
        status_throttle_ms: 0,
        max_pending: 16,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
    }
}

fn real_run_task() -> RunTaskFn {
    Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)))
}

fn task_frame(kind: TaskFrameKind, task_id: &str, text: &str, correlation: Option<&str>) -> String {
    encode_task_frame(EncodeFrameInput {
        kind,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: correlation.map(str::to_string),
        harness: None,
        provider: None,
    })
}

/// Drive one round of the daemon serve loop (drain the worker inbox, dispatch
/// each decoded message) and collect any decrypted frames the owner received.
async fn pump_chain(
    worker: &SignalTransport,
    owner: &SignalTransport,
    runtime: &DaemonRuntime,
    collected: &mut Vec<TaskFrame>,
) {
    for message in worker.drain_inbox(50).await {
        let frame = decode_task_frame(&message.text);
        runtime.handle_message(message.from, message.text, frame);
    }
    for message in owner.drain_inbox(50).await {
        if let Some(frame) = decode_task_frame(&message.text) {
            collected.push(frame);
        }
    }
}

/// Run the chain until `predicate(collected)` holds or the deadline passes.
async fn run_chain_until<F>(
    worker: &SignalTransport,
    owner: &SignalTransport,
    runtime: &DaemonRuntime,
    collected: &mut Vec<TaskFrame>,
    timeout: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut(&[TaskFrame]) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        pump_chain(worker, owner, runtime, collected).await;
        if predicate(collected) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// Scenario 2: full task chain. An owner sends a `medulla-tinyplace/1` task frame;
// the DAEMON receives it over the mock Signal server, admits + runs it on a MOCK
// harness CLI, and the owner receives ack → status → encrypted reply frames,
// each decrypted and validated end-to-end. Stored server payloads are ciphertext.
#[tokio::test]
async fn task_chain_owner_daemon_mock_harness() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-chain", &server.base_url);
    let worker = make_identity("worker-chain", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // The daemon's workspace + a mock claude CLI that emits a rich stream and a
    // final result line the reply is drawn from.
    let work = IdentityDir::new("work-chain");
    let mock = MockCli::new(MockProvider::Claude)
        .thinking("planning the work")
        .tool(
            "read",
            serde_json::json!({ "file_path": "/a/b.rs" }),
            "file contents",
            false,
        )
        .message("intermediate note")
        .claude_result("final answer from worker");
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_CLAUDE_BIN".to_string(), bin);

    let runtime = DaemonRuntime::new(
        daemon_config(
            HarnessProvider::Claude,
            work.path.to_string_lossy().into_owned(),
            env,
        ),
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    // Owner dispatches the task frame (encrypted, opens the X3DH session).
    owner
        .transport
        .send(
            &worker_id,
            &task_frame(TaskFrameKind::Task, "cyc-1", "do the thing", Some("corr-1")),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_reply = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| frames.iter().any(|f| f.kind == TaskFrameKind::Reply),
    )
    .await;
    assert!(
        saw_reply,
        "owner never received a reply frame: {collected:?}"
    );
    runtime.idle().await;
    // Final drain to collect any trailing frames.
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // ack("task accepted") first, reply last, statuses in between.
    assert_eq!(collected[0].kind, TaskFrameKind::Ack);
    assert_eq!(collected[0].text, "task accepted");
    let reply = collected
        .iter()
        .find(|f| f.kind == TaskFrameKind::Reply)
        .expect("reply frame");
    assert_eq!(reply.text, "final answer from worker");
    // At least one status frame arrived (thinking / tool activity). Extra stream
    // records are tolerated — assert presence, not an exact count.
    assert!(
        collected.iter().any(|f| f.kind == TaskFrameKind::Status),
        "expected at least one status frame: {collected:?}"
    );
    // Every frame echoes the correlationId and carries the resolved harness.
    for frame in &collected {
        assert_eq!(frame.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(frame.harness, Some(HarnessProvider::Claude));
        assert_eq!(frame.proto, TINYPLACE_PROTO);
    }

    // Server never saw the task text, the reply, or any status detail in plaintext.
    server.assert_ciphertext_only(&[
        "do the thing",
        "final answer from worker",
        "planning the work",
        "task accepted",
    ]);
}

/// True when a real `opencode` binary is on PATH (drives scenario 3's gate).
fn opencode_available() -> bool {
    std::process::Command::new("opencode")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Scenario 3: the same owner → daemon chain, but the daemon runs the task on the
// REAL `opencode` binary when it is present on PATH. Gated so CI (no opencode)
// stays green: absent → note + early return. When present, a terminal frame
// (reply or error) must come back — the assertion tolerates a keyless
// environment where opencode may fail rather than answer.
#[tokio::test]
async fn task_chain_real_opencode_when_present() {
    if !opencode_available() {
        eprintln!("skipping: opencode not on PATH");
        return;
    }

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-oc", &server.base_url);
    let worker = make_identity("worker-oc", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let work = IdentityDir::new("work-oc");
    // Inherit the real PATH so run_provider_task discovers opencode; no bin override.
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    let mut config = daemon_config(
        HarnessProvider::Opencode,
        work.path.to_string_lossy().into_owned(),
        env,
    );
    // A tight timeout keeps the test fast even if opencode blocks on a missing key.
    config.task_timeout_ms = 20_000;
    let runtime = DaemonRuntime::new(
        config,
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    owner
        .transport
        .send(
            &worker_id,
            &task_frame(
                TaskFrameKind::Task,
                "oc-1",
                "print the word ready",
                Some("oc-c1"),
            ),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_terminal = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        Duration::from_secs(30),
        |frames| {
            frames
                .iter()
                .any(|f| matches!(f.kind, TaskFrameKind::Reply | TaskFrameKind::Error))
        },
    )
    .await;
    runtime.shutdown();
    assert!(
        saw_terminal,
        "real opencode chain produced no terminal frame: {collected:?}"
    );
    // The daemon acked the task before running it, regardless of the outcome.
    assert!(
        collected
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted"),
        "expected a task-accepted ack: {collected:?}"
    );
    // Whatever opencode produced stayed encrypted on the wire.
    server.assert_ciphertext_only(&["print the word ready", "task accepted"]);
}

// ─── wrapper leg ────────────────────────────────────────────────────────────

use medulla::tinyplace_support::{
    encode_harness_control_frame, parse_session_envelope, AnySessionEnvelope, HarnessEventKind,
};
use medulla::wrapper::{run_wrapper_with, WrapperConfig};
use mock_harness::MockCli as WrapperMockCli;

/// The 32-byte seed of a signer as lowercase hex (for `TINYPLACE_SECRET_KEY`).
fn seed_hex(signer: &LocalSigner) -> String {
    signer.seed().iter().map(|b| format!("{b:02x}")).collect()
}

/// Owner-side view of decrypted wrapper session envelopes.
#[derive(Default)]
struct OwnerView {
    messages: Vec<String>,
    phases: Vec<String>,
}

impl OwnerView {
    fn ingest(&mut self, body: &str) {
        if let Some(AnySessionEnvelope::V2(env)) = parse_session_envelope(body) {
            match env.event.decoded() {
                HarnessEventKind::AgentMessage(payload) => self.messages.push(payload.text),
                HarnessEventKind::Lifecycle(payload) => self.phases.push(payload.phase),
                _ => {}
            }
        }
    }
}

async fn drain_owner_until<F>(
    transport: &SignalTransport,
    view: &mut OwnerView,
    timeout: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut(&OwnerView) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        for message in transport.drain_inbox(50).await {
            view.ingest(&message.text);
        }
        if predicate(view) {
            return true;
        }
        if Instant::now() >= deadline {
            return predicate(view);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// Scenario 4: the wrapper bridges a mock-harness session through the mock Signal
// server to an owner listener. Session envelopes arrive encrypted (ciphertext on
// the server), decrypt to valid tinyplace.harness.session.v2, and an inbound
// control input frame reaches the child (echoed back through the transcript).
#[tokio::test]
async fn wrapper_leg_bridges_session_and_injects_control() {
    let server = MockSignalServer::start().await;
    let owner_dir = IdentityDir::new("owner-wrap");
    let owner = {
        let signer = Arc::new(LocalSigner::generate());
        let client = TinyPlaceClient::new(TinyPlaceClientOptions {
            base_url: server.base_url.clone(),
            signer: Some(signer.clone() as Arc<dyn Signer>),
            ..Default::default()
        });
        let transport = SignalTransport::new(client, &signer, &owner_dir.path);
        transport.publish_keys(&signer).await.unwrap();
        (transport, signer)
    };
    let (owner_transport, owner_signer) = owner;
    let owner_id = owner_signer.agent_id();

    // The wrapper's identity: a fixed seed so its agent id is known up front.
    let wrapper_signer = LocalSigner::generate();
    let wrapper_id = wrapper_signer.agent_id();
    let wrapper_seed = seed_hex(&wrapper_signer);

    // Workspace + the codex session log the mock writes and the wrapper tails.
    let work = IdentityDir::new("work-wrap");
    let cwd = work.path.to_string_lossy().into_owned();
    let sessions_dir = work.path.join("codex-sessions");
    let log_path = sessions_dir.join("rollout-signal.jsonl");

    let mock = WrapperMockCli::new(MockProvider::Codex)
        .message("hello from codex")
        .write_session_log(&log_path, "codex-signal", &cwd)
        .echo_stdin_to_log();
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);

    let config_file = work.path.join("tinyplace-config.json");
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_ENDPOINT".to_string(), server.base_url.clone());
    env.insert(
        "TINYPLACE_CONFIG".to_string(),
        config_file.to_string_lossy().into_owned(),
    );
    env.insert("TINYPLACE_SECRET_KEY".to_string(), wrapper_seed);
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin);
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        sessions_dir.to_string_lossy().into_owned(),
    );
    env.insert("TINYPLACE_HARNESS_DM_TO".to_string(), owner_id.clone());
    env.insert(
        "TINYPLACE_HARNESS_RECEIVE_FROM".to_string(),
        owner_id.clone(),
    );

    let wrapper = tokio::spawn(run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd,
        no_bridge: false,
        session_id: Some("tp-codex-signal".to_string()),
    }));

    // The session_start lifecycle + first transcript message arrive, decrypted.
    let mut view = OwnerView::default();
    let saw_hello = drain_owner_until(&owner_transport, &mut view, Duration::from_secs(15), |v| {
        v.messages.iter().any(|m| m == "hello from codex")
    })
    .await;
    assert!(saw_hello, "wrapper never forwarded the transcript message");
    assert!(
        view.phases.iter().any(|p| p == "session_start"),
        "missing session_start lifecycle: {:?}",
        view.phases
    );

    // An owner control frame is injected into the child's stdin; the child echoes
    // it into the transcript, which flows back as another envelope.
    let frame = encode_harness_control_frame("run the suite", None);
    owner_transport.send(&wrapper_id, &frame).await.unwrap();
    let saw_echo = drain_owner_until(&owner_transport, &mut view, Duration::from_secs(15), |v| {
        v.messages.iter().any(|m| m == "got: run the suite")
    })
    .await;
    assert!(saw_echo, "injected control input never reached the child");

    let code = wrapper.await.unwrap().unwrap();
    assert_eq!(code, 0, "wrapper propagates the child's clean exit");
    // The session_end envelope races the wrapper's exit; keep draining for it.
    let saw_end = drain_owner_until(&owner_transport, &mut view, Duration::from_secs(15), |v| {
        v.phases.iter().any(|p| p == "session_end")
    })
    .await;
    assert!(saw_end, "missing session_end lifecycle: {:?}", view.phases);

    // The bridged session content was encrypted on the wire.
    server.assert_ciphertext_only(&["hello from codex", "got: run the suite", "session_start"]);
}

// ─── fault matrix ───────────────────────────────────────────────────────────

use medulla::daemon::providers::RunTaskResult;
use tokio::sync::{mpsc, Notify};

// 5a. A corrupted/re-published bundle triggers the SDK session self-heal: the
// first encrypt rejects the tampered signed pre-key (a session-shaped error), the
// retry re-fetches a valid bundle, and the message still delivers.
#[tokio::test]
async fn fault_corrupted_bundle_self_heals() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-heal", &server.base_url);
    let worker = make_identity("worker-heal", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    server.controls().corrupt_next_bundle();
    owner
        .transport
        .send(&worker.id(), "resilient-payload")
        .await
        .unwrap();

    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "resilient-payload");
    // The self-heal re-fetched a bundle (two fetches: rejected + valid).
    assert!(server.controls().bundle_fetches() >= 2);
}

// 5b. A transient 5xx on `GET /messages` is tolerated by the SDK's retry-with-
// backoff: the two failing attempts are retried inside one `list()` call, the
// third succeeds, and the message still delivers in a single drain. A longer
// outage that exhausts the retry budget yields an empty drain, and the next
// drain (server recovered) delivers it.
#[tokio::test]
async fn fault_5xx_on_list_tolerated_then_delivers() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-5xx", &server.base_url);
    let worker = make_identity("worker-5xx", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Transient outage within the SDK retry budget (GET retries twice): the
    // single drain still delivers, having retried through the 500s.
    owner
        .transport
        .send(&worker_id, "after-retry")
        .await
        .unwrap();
    server.controls().fail_list(2);
    let calls_before = server.controls().list_calls();
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1, "SDK retry delivered through the 5xx");
    assert_eq!(inbox[0].text, "after-retry");
    assert!(
        server.controls().list_calls() - calls_before >= 3,
        "the list call was retried through the 5xx"
    );

    // A longer outage (exceeds the retry budget) yields an empty drain; the next
    // drain, after recovery, delivers the message.
    owner
        .transport
        .send(&worker_id, "after-the-outage")
        .await
        .unwrap();
    server.controls().fail_list(10);
    let inbox = worker.transport.drain_inbox(50).await;
    assert!(
        inbox.is_empty(),
        "an exhausted retry budget yields an empty drain"
    );
    assert_eq!(server.controls().queued_for(&worker_id), 1);
    // Clear the remaining armed failures, then drain succeeds.
    server.controls().fail_list(0);
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "after-the-outage");
}

// 5 (drop bundle). A dropped bundle (404) is NOT a self-healable session error:
// the send surfaces a plain transport error rather than looping.
#[tokio::test]
async fn fault_dropped_bundle_surfaces_non_session_error() {
    use medulla::daemon::transport::is_session_error;

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-drop", &server.base_url);
    let worker = make_identity("worker-drop", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    // Drop the next few bundle fetches (covers the self-heal retry too).
    server.controls().drop_next_bundle(3);
    let err = owner
        .transport
        .send(&worker.id(), "never-arrives")
        .await
        .unwrap_err();
    assert!(
        !is_session_error(&err),
        "a 404 bundle is not self-healable: {err}"
    );
}

// 5 (duplicate delivery). Once a session is established, duplicate delivery of an
// envelope does not double-deliver: the second copy fails to decrypt (its ratchet
// slot is spent) and is skipped, so the drain yields exactly one message.
#[tokio::test]
async fn fault_duplicate_delivery_transport_dedupes() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-dup", &server.base_url);
    let worker = make_identity("worker-dup", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Establish the session with a first (prekey-bundle) message.
    owner.transport.send(&worker_id, "establish").await.unwrap();
    assert_eq!(worker.transport.drain_inbox(50).await.len(), 1);

    // A subsequent ciphertext message, delivered twice.
    owner.transport.send(&worker_id, "dup-me").await.unwrap();
    server.controls().set_duplicate_delivery(true);
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(
        inbox.len(),
        1,
        "duplicate delivery yields one decrypted message"
    );
    assert_eq!(inbox[0].text, "dup-me");
    assert_eq!(
        server.controls().queued_for(&worker_id),
        0,
        "ack drained the queue"
    );
}

// 5 (out-of-order delivery). Two in-chain messages delivered in reverse order
// both decrypt: the double-ratchet's skipped-key mechanism reorders them.
#[tokio::test]
async fn fault_out_of_order_delivery_still_decrypts() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-ooo", &server.base_url);
    let worker = make_identity("worker-ooo", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Establish the session first so both reordered messages are plain ciphertext.
    owner.transport.send(&worker_id, "establish").await.unwrap();
    assert_eq!(worker.transport.drain_inbox(50).await.len(), 1);

    owner.transport.send(&worker_id, "message-A").await.unwrap();
    owner.transport.send(&worker_id, "message-B").await.unwrap();
    server.controls().set_out_of_order(true);

    let inbox = worker.transport.drain_inbox(50).await;
    let texts: Vec<&str> = inbox.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(inbox.len(), 2, "both reordered messages decrypt: {texts:?}");
    assert!(texts.contains(&"message-A"));
    assert!(texts.contains(&"message-B"));
}

/// A runner that signals readiness, then blocks on `gate` before replying.
fn blocking_runner(ready: mpsc::UnboundedSender<()>, gate: Arc<Notify>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let ready = ready.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let _ = ready.send(());
            gate.notified().await;
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

// 5 (taskKey dedupe). A duplicate task frame (same sender + taskId) delivered as
// two separate encrypted envelopes does not double-run: the daemon rejects the
// second with "already running" while the first is still executing.
#[tokio::test]
async fn fault_duplicate_task_frame_no_double_run() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-ddup", &server.base_url);
    let worker = make_identity("worker-ddup", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let runtime = DaemonRuntime::new(
        daemon_config(HarnessProvider::Claude, ".".to_string(), HashMap::new()),
        blocking_runner(ready_tx, gate.clone()),
        transport_send(worker.transport.clone()),
    );

    let dup = task_frame(TaskFrameKind::Task, "dup-1", "one", Some("c-dup"));
    let mut collected: Vec<TaskFrame> = Vec::new();

    // First copy: admitted, runs (blocks). Wait for its ack + the runner's ready.
    owner.transport.send(&worker_id, &dup).await.unwrap();
    let admitted = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| {
            frames
                .iter()
                .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
        },
    )
    .await;
    assert!(admitted, "first task never admitted: {collected:?}");
    tokio::time::timeout(T, ready_rx.recv()).await.unwrap();

    // Second copy of the same frame while the first is still running → rejected.
    owner.transport.send(&worker_id, &dup).await.unwrap();
    let rejected = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| {
            frames
                .iter()
                .any(|f| f.kind == TaskFrameKind::Error && f.text.contains("already running"))
        },
    )
    .await;
    assert!(rejected, "duplicate task was not rejected: {collected:?}");

    // Release the first task and let it settle.
    gate.notify_waiters();
    runtime.idle().await;
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // Exactly one admission, one duplicate rejection, and one reply.
    let accepts = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
        .count();
    let already = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Error && f.text.contains("already running"))
        .count();
    let replies = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Reply)
        .count();
    assert_eq!(accepts, 1, "task admitted exactly once: {collected:?}");
    assert_eq!(already, 1, "duplicate rejected exactly once: {collected:?}");
    assert_eq!(replies, 1, "the task replied exactly once: {collected:?}");
}

// ─── medulla-API fold leg ───────────────────────────────────────────────────

use medulla::runtime::AgentDescriptor;
use medulla_tui::ui::agents::{derive_agent_lanes, TaskStatus};
use medulla_tui::ui::events::{EventEnvelope, TaskDigest, TuiEvent};

/// Fold the frames the owner decrypted off the wire into the TUI event stream
/// the way the owner's UI would surface a delegated task: dispatch → TaskStart,
/// each status → TaskEvent, the reply → TaskComplete.
fn frames_to_events(worker_id: &str, task_id: &str, frames: &[TaskFrame]) -> Vec<EventEnvelope> {
    let mut events = vec![EventEnvelope {
        seq: 0,
        at: 0,
        event: TuiEvent::TaskStart {
            task_id: task_id.to_string(),
            instruction: "do the thing".to_string(),
            depth: 2,
            agent_id: Some(worker_id.to_string()),
        },
    }];
    let mut seq = 1u64;
    for frame in frames {
        match frame.kind {
            TaskFrameKind::Status => {
                events.push(EventEnvelope {
                    seq,
                    at: seq as i64 * 1000,
                    event: TuiEvent::TaskEvent {
                        task_id: task_id.to_string(),
                        event_kind: "text".to_string(),
                        content: frame.text.clone(),
                        harness: frame.harness.map(|h| h.as_str().to_uppercase()),
                    },
                });
                seq += 1;
            }
            TaskFrameKind::Reply => {
                events.push(EventEnvelope {
                    seq,
                    at: seq as i64 * 1000,
                    event: TuiEvent::TaskComplete {
                        digest: TaskDigest {
                            task_id: task_id.to_string(),
                            status: "done".to_string(),
                            digest: frame.text.clone(),
                            result_ref: None,
                            usage: None,
                            depth: 2,
                        },
                    },
                });
                seq += 1;
            }
            _ => {}
        }
    }
    events
}

// Scenario 6 (medulla-API leg): drive the encrypted owner → daemon chain, then
// fold the decrypted frames into TuiEvents and assert they fold, via
// `medulla_tui::ui::agents`, into the expected agents-lane/task state — a worker lane
// for the delegated agent whose task lands Done carrying the reply digest.
#[tokio::test]
async fn decrypted_frames_fold_into_agent_lane_states() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-fold", &server.base_url);
    let worker = make_identity("worker-fold", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let work = IdentityDir::new("work-fold");
    let mock = MockCli::new(MockProvider::Claude)
        .thinking("planning")
        .message("progress note")
        .claude_result("the final result");
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_CLAUDE_BIN".to_string(), bin);
    let runtime = DaemonRuntime::new(
        daemon_config(
            HarnessProvider::Claude,
            work.path.to_string_lossy().into_owned(),
            env,
        ),
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    owner
        .transport
        .send(
            &worker_id,
            &task_frame(
                TaskFrameKind::Task,
                "cyc-9",
                "do the thing",
                Some("corr-fold"),
            ),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_reply = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| frames.iter().any(|f| f.kind == TaskFrameKind::Reply),
    )
    .await;
    assert!(saw_reply, "no reply frame to fold: {collected:?}");
    runtime.idle().await;
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // Fold the decrypted frames through the public agents view-model.
    let roster = vec![AgentDescriptor {
        id: worker_id.clone(),
        name: "worker".to_string(),
        description: String::new(),
        availability: "online".to_string(),
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let events = frames_to_events(&worker_id, "cyc-9", &collected);
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);

    let lane = lanes
        .iter()
        .find(|l| l.key == format!("agent:{worker_id}"))
        .expect("a lane for the delegated worker agent");
    let task = lane
        .tasks
        .iter()
        .find(|t| t.task_id == "cyc-9")
        .expect("the delegated task folded into the lane");
    assert_eq!(task.status, TaskStatus::Done, "reply frame folds to Done");
    // The completion turn carries the reply as its digest content.
    assert!(
        task.turn_blocks
            .iter()
            .any(|b| b.content.as_deref() == Some("the final result")),
        "the reply digest folded into the task's turns"
    );
    // The lane closed out its active task on completion.
    assert_eq!(lane.active_tasks, 0);
}
