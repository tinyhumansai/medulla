//! Shared fixtures for the serving-loop tests, driven over an in-process bridge.
//!
//! These start **real ptys running real shells** and speak the real wire
//! protocol to them; only the transport is swapped, for a `LocalBridgeNetwork`
//! that delivers in memory. So everything the relay actually does — resolving a
//! choice, launching, sampling the emulator, writing input back — is exercised
//! here, and what the Docker suite adds on top is the SSH bootstrap, the UDP
//! link, and two machines.

pub(super) use std::collections::HashMap;
pub(super) use std::sync::Arc;
pub(super) use std::time::{Duration, Instant};

pub(super) use crate::remote::serve::RemoteServer;
pub(super) use medulla::bridge::{Bridge, LocalBridgeNetwork};
pub(super) use medulla::protocol::{
    encode_remote_message, encode_screen_message, parse_remote_message, HarnessProvider,
    RemoteHarnessChoice, RemoteMessage, ScreenMessage,
};

use crate::remote::serve::*;
pub(super) use crate::worker::pty::PtyManager;

/// The client's address on the in-process bridge.
pub(super) const CLIENT: &str = "client";
/// The daemon's.
pub(super) const DAEMON: &str = "daemon";

/// A `LocalSessions` that can start a real shell in a real directory.
pub(super) fn shell_sessions(workspace: &str) -> LocalSessions {
    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: Vec::new(),
        default_provider: HarnessProvider::Shell,
        workspace: workspace.to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        custom_harnesses: Vec::new(),
        budget: None,
        attribution: true,
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        Arc::new(|_| Box::pin(async { Err("unused".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| Box::pin(async {}));
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // Pin the shell so the test does not inherit whatever `$SHELL` the developer
    // happens to run; `sh` exists everywhere this suite does.
    env.insert("MEDULLA_SHELL_BIN".to_string(), "/bin/sh".to_string());
    // A bare prompt, so assertions match output rather than someone's `$PS1`.
    env.insert("PS1".to_string(), "$ ".to_string());
    LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions: PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: workspace.to_string(),
        providers: Vec::new(),
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    }
}

/// A server and the client's side of the bridge between them.
pub(super) fn fixture(workspace: &str) -> (RemoteServer, medulla::bridge::LocalBridge) {
    let network = LocalBridgeNetwork::new();
    let daemon_bridge = network.bind(DAEMON).expect("daemon binds");
    let client_bridge = network.bind(CLIENT).expect("client binds");
    let server = RemoteServer::new(
        shell_sessions(workspace),
        Arc::new(daemon_bridge),
        "test-host".to_string(),
    );
    (server, client_bridge)
}

/// Deliver one client message to the server.
pub(super) async fn say(server: &mut RemoteServer, body: &str) {
    server.handle(CLIENT, body).await;
}

/// Every remote message the client has been sent so far.
pub(super) async fn remote_replies(client: &medulla::bridge::LocalBridge) -> Vec<RemoteMessage> {
    client
        .drain_inbox(64)
        .await
        .iter()
        .filter_map(|message| parse_remote_message(&message.text))
        .collect()
}

/// Spin until `check` passes, sampling screens as we go.
///
/// Real children on real ptys are at the mercy of machine load, so the budget is
/// generous: a tight deadline turns "the box was busy" into a red test.
pub(super) async fn wait_for(
    what: &str,
    server: &mut RemoteServer,
    mut check: impl FnMut(&mut RemoteServer) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check(server) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out after 30s waiting for: {what}");
}

/// The shell row this host offers.
pub(super) fn shell_choice(server: &RemoteServer) -> RemoteHarnessChoice {
    match remote_capabilities(server)
        .harnesses
        .into_iter()
        .find(|choice| choice.provider == "shell")
    {
        Some(choice) => choice,
        None => panic!("this host offers no shell"),
    }
}

pub(super) fn remote_capabilities(server: &RemoteServer) -> medulla::protocol::RemoteCapabilities {
    server.capabilities().clone()
}

/// Open a shell and return its session id.
pub(super) async fn open_shell(
    server: &mut RemoteServer,
    client: &medulla::bridge::LocalBridge,
) -> String {
    let choice = shell_choice(server);
    say(
        server,
        &encode_remote_message(&RemoteMessage::Open {
            request_id: "r1".to_string(),
            harness: choice,
            workspace: String::new(),
            cols: 80,
            rows: 24,
            name: None,
        }),
    )
    .await;
    for message in remote_replies(client).await {
        if let RemoteMessage::Opened { session_id, .. } = message {
            return session_id;
        }
        if let RemoteMessage::OpenFailed { reason, .. } = message {
            panic!("the shell did not start: {reason}");
        }
    }
    panic!("no answer to the open request");
}

/// The whole of a session's screen, as text.
pub(super) fn screen_text(server: &RemoteServer, id: &str) -> String {
    let mut out = String::new();
    server.pty().screen_text_into(id, &mut out);
    out
}
