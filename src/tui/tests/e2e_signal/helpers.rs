//! Shared fixtures for the end-to-end Signal transport suite: temp identity dirs,
//! wired identities against a mock Signal server, poll helpers, and the daemon
//! task-chain harness. Every item is `pub` so the sibling behavior modules
//! (`registration`, `task_chain`, `wrapper`, `fault_matrix`, `fold`) can share
//! it via `use crate::helpers::*;`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::providers::{run_provider_task, RunTaskFn, RunTaskOptions};
use medulla::daemon::transport::SignalTransport;
use medulla::daemon::{DaemonConfig, DaemonRuntime, SendFn};
use medulla::tinyplace::tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};
use medulla::tinyplace::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, HarnessProvider, TaskFrame,
    TaskFrameKind,
};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp dir removed on drop, hosting one identity's on-disk Signal state.
pub struct IdentityDir {
    pub path: PathBuf,
}

impl IdentityDir {
    pub fn new(tag: &str) -> Self {
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
pub struct Identity {
    pub transport: SignalTransport,
    pub signer: Arc<LocalSigner>,
    _dir: IdentityDir,
}

impl Identity {
    pub fn id(&self) -> String {
        self.signer.agent_id()
    }
}

/// Build an identity (fresh wallet) pointed at `base_url`.
pub fn make_identity(tag: &str, base_url: &str) -> Identity {
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
pub async fn poll_until<F>(timeout: Duration, mut predicate: F) -> bool
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

/// Default deadline shared by the polling helpers.
pub const T: Duration = Duration::from_secs(10);

// ─── daemon task-chain harness ──────────────────────────────────────────────

/// A transport-backed [`SendFn`] that encrypts every daemon reply to its peer
/// over the real Signal transport.
pub fn transport_send(transport: SignalTransport) -> SendFn {
    Arc::new(move |to: String, body: String| {
        let transport = transport.clone();
        Box::pin(async move {
            if let Err(err) = transport.send(&to, &body).await {
                eprintln!("mock daemon send failed: {err}");
            }
        })
    })
}

/// Build a daemon config for a single provider against `workspace`/`env`.
pub fn daemon_config(
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

/// The real provider runner (spawns the configured harness CLI).
pub fn real_run_task() -> RunTaskFn {
    Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)))
}

/// Encode a task frame with a fixed timestamp for deterministic tests.
pub fn task_frame(
    kind: TaskFrameKind,
    task_id: &str,
    text: &str,
    correlation: Option<&str>,
) -> String {
    encode_task_frame(EncodeFrameInput {
        kind,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: correlation.map(str::to_string),
        harness: None,
        provider: None,
        model: None,
    })
}

/// Drive one round of the daemon serve loop (drain the worker inbox, dispatch
/// each decoded message) and collect any decrypted frames the owner received.
pub async fn pump_chain(
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
pub async fn run_chain_until<F>(
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
