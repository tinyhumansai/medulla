//! The whole feature, live: a real hub and a real worker, two tiny.place
//! identities, the real relay between them, and a real harness on a pty.
//!
//! Everything else stops one step short of this. The offline end-to-end proves
//! the two halves of the protocol agree, and the live inbox probes prove the
//! transport carries and pushes — but nothing exercised the composition, which
//! is where the interesting failures live: contact gating, Signal sessions
//! between two distinct identities, and whether a subscribe sent from one
//! machine's key resolves against the other's running-task record.
//!
//! `#[ignore]`d: it needs the network and two onboarded identities, which the
//! repo's offline-and-deterministic rule excludes from `cargo test`. Run it:
//!
//! ```sh
//! cargo test -p medulla-tui --test live_screen_stream -- --ignored --nocapture
//! ```
//!
//! Identities come from the machine's existing setup — the worker from
//! `~/.tinyplace/config.json` and the hub from `~/.medulla/tinyplace-hub/`,
//! which is where `medulla` puts them. It skips rather than fails when either
//! is missing, since a machine that never onboarded has nothing to prove.
//!
//! Unix-only: it runs `/bin/sh` on a pty.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::transport::SignalTransport;
use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::hub::{Relay, TaskRunner};
use medulla::tinyplace::{
    encode_task_frame, load_or_create_identity, parse_screen_message, resolve_endpoint,
    EncodeFrameInput, HarnessProvider, TaskFrameKind,
};
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};
use medulla_tui::worker::stream::{send_fn, ScreenRouter};
// The SDK re-exports the tiny.place crate, so the app crate reaches it here
// rather than taking a direct dependency it does not otherwise need.
use medulla::tinyplace::tinyplace::{
    auth, LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions,
};

/// How long to allow for the whole round trip across the public internet.
const PATIENCE: Duration = Duration::from_secs(90);

/// Load an identity and a client for it from `dir/config.json`.
fn identity(dir: PathBuf, label: &str) -> Option<(TinyPlaceClient, Arc<LocalSigner>, PathBuf)> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let config_file = dir.join("config.json");
    if !config_file.exists() {
        eprintln!("skipping: no {label} identity at {}", config_file.display());
        return None;
    }
    let (signer, config) = load_or_create_identity(&config_file, &env).ok()?;
    let base_url = resolve_endpoint(&env, &config);
    let signer = Arc::new(signer);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url,
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    eprintln!("{label}: {}", signer.agent_id());
    Some((client, signer, dir))
}

/// A spec that runs `sh -c <script>` on a pty, standing in for a harness.
fn sh(script: &str, label: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        // Codex: claude's interactive argv carries a `--session-id` that
        // `/bin/sh` would reject.
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: label.to_string(),
        session_id: None,
        model: None,
        control: HarnessControl::Orchestrator,
        user_spawned: false,
    }
}

/// A worker runtime whose executor reports the pty it serves and stays running,
/// which is what a task looks like while it is being watched.
fn worker_runtime(session_id: String) -> DaemonRuntime {
    let config = DaemonConfig {
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/tmp".into(),
        env: HashMap::new(),
        task_timeout_ms: 300_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 300_000,
        max_pending: 4,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        accessible_dirs: Vec::new(),
        router: None,
        custom_harnesses: Vec::new(),
        budget: None,
    };
    let run_task = Arc::new(move |options: medulla::daemon::providers::RunTaskOptions| {
        let session_id = session_id.clone();
        Box::pin(async move {
            if let Some(report) = options.on_session {
                report(session_id);
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
            Err("never settles".to_string())
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
    }) as medulla::daemon::providers::RunTaskFn;
    DaemonRuntime::new(config, run_task, Arc::new(|_, _| Box::pin(async {})))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the network and two onboarded identities; run with --ignored"]
async fn a_hub_watches_a_real_workers_screen_over_the_relay() {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    // The worker is whichever identity `medulla` itself resolves to, so this
    // watches the machine the hub's roster actually names rather than some
    // other key that happens to be on disk.
    let worker_config = medulla::tinyplace::config_path(
        &std::env::vars().collect::<HashMap<String, String>>(),
        &home,
    );
    let Some((worker_client, worker_signer, worker_dir)) = identity(
        worker_config
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".tinyplace")),
        "worker",
    ) else {
        return;
    };
    let Some((hub_client, hub_signer, hub_dir)) =
        identity(home.join(".medulla/tinyplace-hub"), "hub")
    else {
        return;
    };

    let worker_addr = worker_signer.agent_id();
    let hub_addr = hub_signer.agent_id();
    assert_ne!(
        worker_addr, hub_addr,
        "these must be two distinct identities"
    );

    let worker_tx = SignalTransport::new(worker_client, &worker_signer, &worker_dir);
    let hub_tx = SignalTransport::new(hub_client, &hub_signer, &hub_dir);

    // Both ends need publishable keys before either can open a Signal session.
    worker_tx
        .publish_keys(&worker_signer)
        .await
        .expect("the worker should be able to publish its keys");
    hub_tx
        .publish_keys(&hub_signer)
        .await
        .expect("the hub should be able to publish its keys");

    // The relay refuses a DM between non-contacts, so open the edge both ways
    // and let each side's accept settle.
    let _ = hub_tx.request_contact(&worker_addr).await;
    let _ = worker_tx.request_contact(&hub_addr).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !hub_tx.contact_accepted(&worker_addr).await {
        let _ = worker_client_accept(&worker_tx, &hub_addr).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("✓ contact edge open");

    // Open the push channel on both ends. Without these the run would prove
    // only that polling works — which it did before this existed.
    let _worker_push = worker_tx.spawn_inbox_listener(None);
    let _hub_push = hub_tx.spawn_inbox_listener(None);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline
        && !(worker_tx.is_push_listening() && hub_tx.is_push_listening())
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        worker_tx.is_push_listening() && hub_tx.is_push_listening(),
        "both ends should have an open push channel"
    );
    eprintln!("✓ push channels open on both ends");

    // ---- worker side -------------------------------------------------------
    let sessions = PtyManager::new();
    let pty = sessions
        .open(sh(
            // The second paint lands well after the subscribe, so it can only
            // reach the hub as a delta against the full frame that carried the
            // first.
            "printf 'LIVE-HARNESS-UP\\n'; sleep 15; printf 'SECOND-PAINT\\n'; sleep 240",
            &hub_addr,
        ))
        .expect("a pty session");
    let runtime = worker_runtime(pty);
    let mut router = ScreenRouter::new(sessions.clone(), runtime.clone(), {
        let tx = worker_tx.clone();
        send_fn(move |to: String, body: String| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(&to, &body).await;
            }
        })
    });

    let worker_loop = {
        let worker_tx = worker_tx.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            loop {
                for message in worker_tx.drain_inbox(50).await {
                    if let Some(screen) = parse_screen_message(&message.text) {
                        router.handle(&message.from, screen);
                        continue;
                    }
                    let frame = medulla::tinyplace::decode_task_frame(&message.text);
                    runtime.handle_message(message.from, message.text, frame);
                }
                worker_tx.wait_for_inbox(Duration::from_millis(500)).await;
            }
        })
    };

    // ---- hub side ----------------------------------------------------------
    // `Relay` is now the shared `Bridge` contract; the tiny.place endpoint
    // wraps the transport rather than implementing it directly.
    let relay: Arc<dyn Relay> = Arc::new(medulla::bridge::TinyplaceBridge::new(hub_tx.clone()));
    let runner = TaskRunner::start(relay, Duration::from_millis(500));
    let screens = runner.screens();

    // Dispatch a task, so the worker has a running record for the hub to name.
    let task_id = format!("live-{}", std::process::id());
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: task_id.clone(),
        text: "hold a session open so it can be watched".to_string(),
        ts: auth::timestamp(),
        correlation_id: Some(format!("live/{task_id}/0")),
        harness: None,
        provider: Some(HarnessProvider::Codex),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        conversation: None,
    });
    hub_tx
        .send(&worker_addr, &body)
        .await
        .expect("the hub should be able to dispatch to the worker");
    eprintln!("✓ task {task_id} dispatched");

    // Give the worker a moment to admit it and record its session, then watch.
    tokio::time::sleep(Duration::from_secs(3)).await;
    // Same message `HubHandle::watch` sends; the runner owns the store the pump
    // folds into, and the bridge is what carries it.
    hub_tx
        .send(
            &worker_addr,
            &medulla::tinyplace::encode_screen_message(
                &medulla::tinyplace::ScreenMessage::Subscribe {
                    task_id: task_id.clone(),
                    max_fps: 1,
                    resync: true,
                },
            ),
        )
        .await
        .expect("the subscribe should send");
    eprintln!("✓ subscribed");

    // ---- the assertion -----------------------------------------------------
    let deadline = Instant::now() + PATIENCE;
    let mut first_seq: Option<i64> = None;
    let mut second_seq: Option<i64> = None;
    while Instant::now() < deadline && second_seq.is_none() {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Some(held) = screens.get(&worker_addr, &task_id) else {
            continue;
        };
        let text: String = held
            .grid
            .lines
            .iter()
            .flatten()
            .map(|run| run.text.as_str())
            .collect();
        if first_seq.is_none() && text.contains("LIVE-HARNESS-UP") {
            eprintln!(
                "✓ full frame: the hub holds the worker's screen, seq {} at {}x{}",
                held.seq, held.grid.cols, held.grid.rows
            );
            show(&held);
            first_seq = Some(held.seq);
        }
        // The second paint happened long after the subscription, so reaching the
        // hub at all means a *delta* was produced, sent, and applied on top of
        // what was already held.
        if first_seq.is_some() && text.contains("SECOND-PAINT") {
            eprintln!(
                "✓ delta applied: seq {} now carries the later paint",
                held.seq
            );
            show(&held);
            second_seq = Some(held.seq);
        }
    }

    worker_loop.abort();
    sessions.shutdown();

    // Both ends were pushing throughout, and a drain skips the HTTPS fetch
    // while the socket is healthy and recently reconciled. The whole run is
    // shorter than that reconciliation window, so everything after the first
    // drain reached its destination *over the socket*.
    assert!(
        hub_tx.is_push_listening(),
        "the hub's push channel should have stayed open for the whole run"
    );

    let first = first_seq.expect("the hub never received the worker's screen");
    let second = second_seq.expect("the hub never received the later paint");
    assert!(
        second > first,
        "the delta should have advanced the sequence ({first} → {second})"
    );
}

/// Print the non-blank rows of a held screen, so a run shows what the hub sees.
fn show(held: &medulla::hub::WatchedScreen) {
    for row in &held.grid.lines {
        let line: String = row.iter().map(|r| r.text.as_str()).collect();
        if !line.trim().is_empty() {
            eprintln!("    │ {line}");
        }
    }
}

/// Accept any pending contact request addressed to this transport's wallet.
async fn worker_client_accept(worker: &SignalTransport, peer: &str) -> Result<(), String> {
    // `request_contact` is idempotent and settles the reverse edge, which is all
    // that is needed for the relay to allow the DM.
    worker.request_contact(peer).await
}
