//! Wrapper leg: the wrapper bridges a mock-harness session through the mock
//! Signal server to an owner listener. Session envelopes arrive encrypted,
//! decrypt to valid `tinyplace.harness.session.v2`, and an inbound control input
//! frame reaches the child (echoed back through the transcript).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::transport::SignalTransport;
use medulla::tinyplace::tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};
use medulla::tinyplace::{
    encode_harness_control_frame, parse_session_envelope, AnySessionEnvelope, HarnessEventKind,
    HarnessProvider,
};
use medulla::wrapper::{run_wrapper_with, WrapperConfig};

use crate::helpers::*;
use crate::mock_harness::{MockCli as WrapperMockCli, MockDir, MockProvider};
use crate::mock_signal_server::MockSignalServer;

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
