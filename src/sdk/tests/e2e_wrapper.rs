//! (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
#![cfg(unix)]

//! End-to-end coverage for the transparent harness wrapper
//! ([`medulla::wrapper`]) with no network and no real provider CLI.
//!
//! A mock codex ([`mock_harness`]) writes a session-log transcript and blocks on
//! stdin. The wrapper spawns it, tails the transcript, and forwards each record
//! as an encrypted v2 envelope to an "owner" identity over an in-memory
//! [`MockRelay`]. The owner then sends an input control frame back; the wrapper
//! injects it into the child's stdin, the child echoes it into its transcript,
//! and that echo flows back as another envelope — proving the full outbound +
//! inbound bridge. Passthrough (`--no-bridge`) and exit-code propagation are
//! covered separately.

mod support;

#[path = "support/mock_harness.rs"]
mod mock_harness;
#[path = "support/mock_harness_relay.rs"]
mod mock_harness_relay;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::transport::SignalTransport;
use medulla::tinyplace::tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};
use medulla::tinyplace::{
    encode_harness_control_frame, parse_session_envelope, AnySessionEnvelope, HarnessEventKind,
    HarnessProvider,
};
use medulla::wrapper::{run_wrapper_with, WrapperConfig};

use mock_harness::{MockCli, MockDir, MockProvider};
use mock_harness_relay::MockRelay;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp dir removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "medulla-wrapper-e2e-{tag}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn seed_hex(signer: &LocalSigner) -> String {
    signer.seed().iter().map(|b| format!("{b:02x}")).collect()
}

/// Build an owner identity: a transport pointed at `base_url` with keys published.
async fn make_owner(base_url: &str, dir: &TempDir) -> (SignalTransport, Arc<LocalSigner>) {
    let signer = Arc::new(LocalSigner::generate());
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.to_string(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let transport = SignalTransport::new(client, &signer, &dir.path);
    transport.publish_keys(&signer).await.unwrap();
    (transport, signer)
}

/// Collected agent-message texts + lifecycle phases from envelopes drained so far.
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

/// Drain the owner inbox until `predicate(view)` holds or the deadline passes.
async fn drain_until<F>(
    transport: &SignalTransport,
    view: &mut OwnerView,
    timeout: Duration,
    mut predicate: F,
) -> bool
where
    F: FnMut(&OwnerView) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for message in transport.drain_inbox(50).await {
            view.ingest(&message.text);
        }
        if predicate(view) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    predicate(view)
}

#[tokio::test]
async fn bridges_transcript_and_injects_owner_input() {
    let relay = MockRelay::start().await;
    let owner_dir = TempDir::new("owner");
    let (owner, owner_signer) = make_owner(&relay.base_url, &owner_dir).await;
    let owner_id = owner_signer.agent_id();

    // The wrapper's identity (deterministic seed → known agent id).
    let wrapper_signer = LocalSigner::generate();
    let wrapper_id = wrapper_signer.agent_id();
    let wrapper_seed = seed_hex(&wrapper_signer);

    // Workspace + sessions dir the mock writes into and the wrapper discovers.
    let work = TempDir::new("work");
    let cwd = work.path.to_string_lossy().into_owned();
    let sessions_dir = work.path.join("codex-sessions");
    let log_path = sessions_dir.join("rollout-e2e.jsonl");

    let mock = MockCli::new(MockProvider::Codex)
        .message("hello from codex")
        .write_session_log(&log_path, "codex-e2e", &cwd)
        .echo_stdin_to_log();
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);

    let config_file = work.path.join("tinyplace-config.json");
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_ENDPOINT".to_string(), relay.base_url.clone());
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

    // Run the wrapper; it blocks in the child's `read` until we inject input.
    let wrapper = tokio::spawn(run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd,
        no_bridge: false,
        session_id: Some("tp-codex-e2e".to_string()),
        pty_spawner: None,
    }));

    // The session_start lifecycle and the first transcript message arrive.
    let mut view = OwnerView::default();
    let saw_hello = drain_until(&owner, &mut view, Duration::from_secs(15), |v| {
        v.messages.iter().any(|m| m == "hello from codex")
    })
    .await;
    assert!(saw_hello, "wrapper never forwarded the transcript message");
    assert!(
        view.phases.iter().any(|p| p == "session_start"),
        "missing session_start lifecycle: {:?}",
        view.phases
    );

    // Send an input control frame; the wrapper injects it into the child's stdin.
    let frame = encode_harness_control_frame("run tests", None);
    owner.send(&wrapper_id, &frame).await.unwrap();

    // The child echoes the injected input into the transcript, which flows back.
    let saw_echo = drain_until(&owner, &mut view, Duration::from_secs(15), |v| {
        v.messages.iter().any(|m| m == "got: run tests")
    })
    .await;
    assert!(saw_echo, "injected input never reached the child's stdin");

    let code = wrapper.await.unwrap().unwrap();
    assert_eq!(code, 0, "wrapper should propagate the child's clean exit");
    // The session_end envelope races the wrapper's exit; keep draining for it.
    let saw_end = drain_until(&owner, &mut view, Duration::from_secs(15), |v| {
        v.phases.iter().any(|p| p == "session_end")
    })
    .await;
    assert!(saw_end, "missing session_end lifecycle: {:?}", view.phases);
}

/// A per-provider `TINYPLACE_CODEX_DM_TO` beats the generic `TINYPLACE_HARNESS_DM_TO`
/// end-to-end: with a decoy generic recipient and the real owner set per-provider,
/// envelopes must reach the real owner.
#[tokio::test]
async fn per_provider_dm_to_beats_generic() {
    let relay = MockRelay::start().await;
    let owner_dir = TempDir::new("owner-pp");
    let (owner, owner_signer) = make_owner(&relay.base_url, &owner_dir).await;
    let owner_id = owner_signer.agent_id();

    let wrapper_signer = LocalSigner::generate();
    let wrapper_seed = seed_hex(&wrapper_signer);

    let work = TempDir::new("work-pp");
    let cwd = work.path.to_string_lossy().into_owned();
    let sessions_dir = work.path.join("codex-sessions");
    let log_path = sessions_dir.join("rollout-pp.jsonl");

    let mock = MockCli::new(MockProvider::Codex)
        .message("hello via per-provider")
        .write_session_log(&log_path, "codex-pp", &cwd);
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);

    let config_file = work.path.join("tinyplace-config.json");
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_ENDPOINT".to_string(), relay.base_url.clone());
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
    // Decoy generic recipient (a bogus id nobody drains) + real per-provider owner.
    env.insert(
        "TINYPLACE_HARNESS_DM_TO".to_string(),
        "decoy-generic-owner".to_string(),
    );
    env.insert("TINYPLACE_CODEX_DM_TO".to_string(), owner_id.clone());
    // No inbound receive for this test.
    env.insert("TINYPLACE_CODEX_RECEIVE".to_string(), "0".to_string());

    let wrapper = tokio::spawn(run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd,
        no_bridge: false,
        session_id: Some("tp-codex-pp".to_string()),
        pty_spawner: None,
    }));

    let mut view = OwnerView::default();
    let saw = drain_until(&owner, &mut view, Duration::from_secs(15), |v| {
        v.messages.iter().any(|m| m == "hello via per-provider")
    })
    .await;
    assert!(
        saw,
        "per-provider DM_TO owner never received the transcript message"
    );

    let code = wrapper.await.unwrap().unwrap();
    assert_eq!(code, 0);
}

#[tokio::test]
async fn passthrough_propagates_exit_code() {
    // --no-bridge: no relay, no envelopes — just run the child and return its code.
    let mock = MockCli::new(MockProvider::Codex).fail(7, "boom");
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);

    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_CODEX_BIN".to_string(), bin);

    let code = run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd: ".".to_string(),
        no_bridge: true,
        session_id: Some("tp-codex-passthrough".to_string()),
        pty_spawner: None,
    })
    .await
    .unwrap();
    assert_eq!(code, 7);
}

#[tokio::test]
async fn missing_binary_errors_clearly() {
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("PATH".to_string(), "/nonexistent-dir".to_string());
    env.insert(
        "TINYPLACE_CODEX_BIN".to_string(),
        "/no/such/codex".to_string(),
    );
    let err = run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd: ".".to_string(),
        no_bridge: true,
        session_id: None,
        pty_spawner: None,
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not found on PATH"), "got: {err}");
}
