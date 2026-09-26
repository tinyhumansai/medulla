//! Client and server talking to each other, over an in-process bridge.
//!
//! The `serve` suite drives the daemon with hand-built messages; this one puts
//! the *real client* on the other end, so the two halves are checked against each
//! other rather than each against my idea of the other. Only the transport is
//! swapped — everything else, down to the shell on the pty, is real.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::bridge::LocalBridgeNetwork;
use medulla::protocol::HarnessProvider;

use super::*;
use crate::remote::serve::RemoteServer;
use crate::ui::harness_pane::LocalSessions;
use crate::worker::pty::PtyManager;

/// A `LocalSessions` that can start a real `/bin/sh`.
fn shell_sessions(workspace: &str) -> LocalSessions {
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
    env.insert("MEDULLA_SHELL_BIN".to_string(), "/bin/sh".to_string());
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

/// A connected client and the server it talks to.
fn pair() -> (RemoteHost, RemoteServer) {
    let network = LocalBridgeNetwork::new();
    // The names have to match what each side addresses the other by.
    let daemon_bridge = network.bind("medulla-host").expect("daemon binds");
    let client_bridge = network.bind("medulla-client").expect("client binds");
    let server = RemoteServer::new(
        shell_sessions("/tmp"),
        Arc::new(daemon_bridge),
        "docker-host".to_string(),
    );
    let client = RemoteHost::over("tower", Arc::new(client_bridge));
    (client, server)
}

/// Let both sides make progress until `check` passes.
async fn settle(
    what: &str,
    client: &mut RemoteHost,
    server: &mut RemoteServer,
    mut check: impl FnMut(&RemoteHost) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        server.pump_once().await;
        server.publish_screens_once().await;
        client.pump().await;
        if check(client) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out after 30s waiting for: {what}");
}

#[tokio::test]
async fn a_client_learns_what_the_host_offers() {
    let (mut client, mut server) = pair();
    client.hello().await.unwrap();
    settle("capabilities", &mut client, &mut server, |client| {
        !client.capabilities().harnesses.is_empty()
    })
    .await;
    assert_eq!(client.capabilities().host_name, "docker-host");
    assert!(client
        .capabilities()
        .harnesses
        .iter()
        .any(|choice| choice.provider == "shell"));
}

#[tokio::test]
async fn a_client_opens_a_shell_types_into_it_and_sees_the_output() {
    // The whole feature, end to end, minus SSH and UDP: open a session on
    // another "machine", type a command, and read the result out of a screen
    // folded from frames. The Docker suite runs this same shape over a real
    // network.
    let (mut client, mut server) = pair();
    client.hello().await.unwrap();
    settle("capabilities", &mut client, &mut server, |client| {
        !client.capabilities().harnesses.is_empty()
    })
    .await;

    let shell = client
        .capabilities()
        .harnesses
        .iter()
        .find(|choice| choice.provider == "shell")
        .expect("a shell")
        .id
        .clone();
    client.open("r1", &shell, "", 80, 24, None).await.unwrap();

    let mut session_id = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && session_id.is_empty() {
        server.pump_once().await;
        for event in client.pump().await {
            if let PumpEvent::Opened(id) = event {
                session_id = id;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!session_id.is_empty(), "the host never confirmed the open");

    client.watch(&session_id).await.unwrap();
    settle("the subscription to land", &mut client, &mut server, |_| {
        true
    })
    .await;
    client.input(b"echo docker-e2e-marker\n").await.unwrap();

    settle(
        "the marker to appear on the client's screen",
        &mut client,
        &mut server,
        |client| screen_text(client).contains("docker-e2e-marker"),
    )
    .await;

    // And the row for it arrived on the reliable channel.
    assert!(client.rows().iter().any(|row| row.id == session_id));
}

#[tokio::test]
async fn watching_a_second_session_moves_the_stream() {
    let (mut client, mut server) = pair();
    client.hello().await.unwrap();
    settle("capabilities", &mut client, &mut server, |client| {
        !client.capabilities().harnesses.is_empty()
    })
    .await;
    let shell = client.capabilities().harnesses[0].id.clone();

    let mut ids: Vec<String> = Vec::new();
    for (index, request) in ["r1", "r2"].into_iter().enumerate() {
        client
            .open(request, &shell, "", 80, 24, None)
            .await
            .unwrap();
        let wanted = index + 1;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && ids.len() < wanted {
            server.pump_once().await;
            ids.extend(
                client
                    .pump()
                    .await
                    .into_iter()
                    .filter_map(|event| match event {
                        PumpEvent::Opened(id) => Some(id),
                        PumpEvent::Connected | PumpEvent::OpenFailed(_) => None,
                    }),
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    assert_eq!(ids.len(), 2, "two sessions should have opened");

    client.watch(&ids[0]).await.unwrap();
    settle("the first subscription", &mut client, &mut server, |_| true).await;
    client.watch(&ids[1]).await.unwrap();
    settle("the second subscription", &mut client, &mut server, |_| {
        true
    })
    .await;

    assert_eq!(client.watching(), Some(ids[1].as_str()));
    assert_eq!(
        server.watching(),
        1,
        "the host streams one screen per client"
    );
}

#[tokio::test]
async fn a_refused_open_reports_why_it_failed() {
    // Before `PumpEvent::OpenFailed` existed, `pump` parsed `OpenFailed`'s
    // `reason` off the wire and threw it away — a caller waiting for
    // `PumpEvent::Opened` had nothing to distinguish "still starting" from
    // "already refused, will never arrive" except a 30s timeout.
    let (mut client, mut server) = pair();
    client.hello().await.unwrap();
    settle("capabilities", &mut client, &mut server, |client| {
        !client.capabilities().harnesses.is_empty()
    })
    .await;

    let shell = client.capabilities().harnesses[0].id.clone();
    client
        // Advertised, so the client's own local check does not refuse it — the
        // point is what happens after the request actually reaches the host.
        .open("r1", &shell, "/no/such/directory", 80, 24, None)
        .await
        .unwrap();

    let mut failure = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && failure.is_none() {
        server.pump_once().await;
        for event in client.pump().await {
            if let PumpEvent::OpenFailed(reason) = event {
                failure = Some(reason);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let reason = failure.expect("the refusal must reach the caller, not just time out");
    assert!(
        reason.contains("directory"),
        "the host's actual reason must survive: {reason}"
    );
}

/// The watched screen as text.
fn screen_text(client: &RemoteHost) -> String {
    client
        .screen()
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
