//! Unit tests for the embedded host: start-up validation, device-local task
//! round-trips, and the observation counters a UI renders.
//!
//! Everything here is offline and process-free. Provider *detection* is made
//! deterministic through the documented `TINYPLACE_*_BIN` overrides pointed at a
//! binary every test machine has, and the executor is substituted so no agent
//! CLI is ever spawned.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::bridge::{Bridge, LocalBridge, LocalBridgeNetwork};
use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::tinyplace::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, HarnessProvider, TaskFrame,
    TaskFrameKind,
};

use super::{accessible_dirs, EmbeddedDaemon, EmbeddedDaemonOptions};

/// A path that is guaranteed to exist and be executable on every platform.
///
/// The running test binary itself. `/bin/sh` was the obvious choice and the
/// wrong one: it does not exist on Windows, so detection found nothing there and
/// every test that needed an "installed" harness failed on that runner alone.
fn installed_bin() -> String {
    std::env::current_exe()
        .expect("the test binary has a path")
        .to_string_lossy()
        .into_owned()
}

/// An environment in which exactly `claude` is "installed".
///
/// `PATH` is emptied so nothing actually on this machine can be discovered, and
/// the bin override names an absolute path — which detection probes directly for
/// executability — so the result does not depend on the host's PATH at all.
fn env_with_only_claude() -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), String::new()),
        ("TINYPLACE_CLAUDE_BIN".to_string(), installed_bin()),
    ])
}

/// An environment with no coding-agent CLI at all.
fn env_with_nothing() -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), String::new()),
        (
            "TINYPLACE_CLAUDE_BIN".to_string(),
            "/nonexistent/claude".to_string(),
        ),
        (
            "TINYPLACE_CODEX_BIN".to_string(),
            "/nonexistent/codex".to_string(),
        ),
        (
            "TINYPLACE_OPENCODE_BIN".to_string(),
            "/nonexistent/opencode".to_string(),
        ),
    ])
}

/// Options that detect only `claude` and poll fast enough for a test.
fn options(env: HashMap<String, String>) -> EmbeddedDaemonOptions {
    EmbeddedDaemonOptions {
        env,
        poll: Duration::from_millis(5),
        ..Default::default()
    }
}

/// An executor that answers every task with `reply` without spawning anything.
fn replying_executor(reply: &'static str) -> RunTaskFn {
    Arc::new(move |options: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                provider: options.provider,
                reply: reply.to_string(),
                events: 1,
                usage: None,
                session_id: None,
            })
        })
    })
}

/// An executor that always fails, for the error-counter path.
fn failing_executor() -> RunTaskFn {
    Arc::new(|_: RunTaskOptions| Box::pin(async { Err("harness exploded".to_string()) }))
}

/// Start a host on a fresh local network, returning it with a peer endpoint.
fn hosted(run_task: RunTaskFn) -> (EmbeddedDaemon, LocalBridge) {
    let network = LocalBridgeNetwork::new();
    let host_bridge = network.bind("host").unwrap();
    let peer = network.bind("peer").unwrap();
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(host_bridge) as Arc<dyn Bridge>,
        "host",
        options(env_with_only_claude()),
        run_task,
    )
    .expect("host starts with claude detected");
    (host, peer)
}

/// A `task` frame addressed to the host.
fn task_frame(task_id: &str, prompt: &str) -> String {
    encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: prompt.to_string(),
        ts: "T".to_string(),
        correlation_id: Some(format!("corr-{task_id}")),
        harness: None,
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    })
}

/// Everything a peer has received so far.
///
/// A buffer rather than a bare drain helper because the inbox is *destructive*
/// and one drain pass routinely yields several frames at once — a helper that
/// returned on the first match would throw the rest away, and the terminal
/// `reply` would be lost exactly when the `ack` arrived beside it.
struct Received {
    peer: LocalBridge,
    frames: Vec<TaskFrame>,
}

impl Received {
    fn new(peer: LocalBridge) -> Self {
        Self {
            peer,
            frames: Vec::new(),
        }
    }

    /// Drain whatever is queued into the buffer.
    async fn pump(&mut self) {
        for message in self.peer.drain_inbox(50).await {
            if let Some(frame) = decode_task_frame(&message.text) {
                self.frames.push(frame);
            }
        }
    }

    /// Wait for a frame of `kind`, polling until a deadline.
    ///
    /// A deadline rather than a fixed sleep: the host drains on its own interval,
    /// so a fixed wait is either flaky or needlessly slow.
    async fn wait_for(&mut self, kind: TaskFrameKind) -> TaskFrame {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            self.pump().await;
            if let Some(frame) = self.frames.iter().find(|frame| frame.kind == kind) {
                return frame.clone();
            }
            if tokio::time::Instant::now() >= deadline {
                let seen: Vec<_> = self.frames.iter().map(|frame| frame.kind).collect();
                panic!("no {kind:?} frame within the deadline; saw {seen:?}");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Whether a frame of `kind` has been seen.
    fn seen(&self, kind: TaskFrameKind) -> bool {
        self.frames.iter().any(|frame| frame.kind == kind)
    }
}

#[tokio::test]
async fn a_task_dispatched_locally_is_acked_and_replied_to_without_any_network() {
    let (host, peer) = hosted(replying_executor("done on this device"));

    peer.send("host", &task_frame("t1", "do the thing"))
        .await
        .unwrap();

    let mut received = Received::new(peer);
    let ack = received.wait_for(TaskFrameKind::Ack).await;
    assert_eq!(ack.task_id, "t1");
    let reply = received.wait_for(TaskFrameKind::Reply).await;
    assert_eq!(reply.text, "done on this device");
    assert_eq!(reply.correlation_id.as_deref(), Some("corr-t1"));

    host.idle().await;
    let stats = host.stats();
    assert_eq!(stats.tasks_started, 1);
    assert_eq!(stats.tasks_completed, 1);
    assert_eq!(stats.tasks_failed, 0);
    assert_eq!(stats.tasks_running(), 0);
    assert_eq!(stats.last_peer.as_deref(), Some("peer"));
    assert!(stats.received >= 1);
}

#[tokio::test]
async fn a_failing_task_is_counted_as_failed_and_reported_to_the_peer() {
    let (host, peer) = hosted(failing_executor());

    peer.send("host", &task_frame("t2", "do the impossible"))
        .await
        .unwrap();

    let error = Received::new(peer).wait_for(TaskFrameKind::Error).await;
    assert!(
        error.text.contains("harness exploded"),
        "unexpected error text: {}",
        error.text
    );

    host.idle().await;
    let stats = host.stats();
    assert_eq!(stats.tasks_failed, 1);
    assert_eq!(stats.tasks_completed, 0);
    assert_eq!(stats.tasks_running(), 0);
}

#[tokio::test]
async fn the_host_answers_a_capability_probe_with_what_this_machine_has() {
    let (host, peer) = hosted(replying_executor("unused"));

    peer.send(
        "host",
        &encode_task_frame(EncodeFrameInput {
            kind: TaskFrameKind::Capabilities,
            task_id: "probe".to_string(),
            text: String::new(),
            ts: "T".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        }),
    )
    .await
    .unwrap();

    let result = Received::new(peer)
        .wait_for(TaskFrameKind::CapabilitiesResult)
        .await;
    assert!(
        result.text.contains("claude"),
        "capabilities should name the detected provider: {}",
        result.text
    );
    host.idle().await;
    assert_eq!(host.stats().probes_answered, 1);
}

#[tokio::test]
async fn a_host_reports_what_it_detected_and_where_tasks_run() {
    let (host, _peer) = hosted(replying_executor("unused"));

    assert_eq!(host.address(), "host");
    assert_eq!(host.providers(), [HarnessProvider::Claude]);
    assert_eq!(host.default_provider(), HarnessProvider::Claude);
    // The workspace defaults to the process's directory, resolved absolutely so
    // an operator can tell exactly what a peer would be editing.
    assert!(std::path::Path::new(host.workspace()).is_absolute());
}

#[tokio::test]
async fn a_machine_with_no_agent_cli_refuses_to_host_rather_than_rejecting_every_task() {
    let network = LocalBridgeNetwork::new();
    let bridge = Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>;

    let error = EmbeddedDaemon::start_with_executor(
        bridge,
        "host",
        options(env_with_nothing()),
        replying_executor("unused"),
    )
    .unwrap_err();

    assert!(
        error.contains("no coding-agent CLI"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn requesting_a_provider_this_machine_lacks_fails_at_start_up() {
    let network = LocalBridgeNetwork::new();
    let bridge = Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>;

    let error = EmbeddedDaemon::start_with_executor(
        bridge,
        "host",
        EmbeddedDaemonOptions {
            providers: Some(vec![HarnessProvider::Codex]),
            ..options(env_with_only_claude())
        },
        replying_executor("unused"),
    )
    .unwrap_err();

    assert!(error.contains("codex"), "unexpected error: {error}");
}

#[tokio::test]
async fn a_default_provider_that_is_not_installed_is_rejected_by_name() {
    let network = LocalBridgeNetwork::new();
    let bridge = Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>;

    let error = EmbeddedDaemon::start_with_executor(
        bridge,
        "host",
        EmbeddedDaemonOptions {
            default_provider: Some(HarnessProvider::Opencode),
            ..options(env_with_only_claude())
        },
        replying_executor("unused"),
    )
    .unwrap_err();

    assert!(error.contains("opencode"), "unexpected error: {error}");
    assert!(
        error.contains("claude"),
        "the error should name what IS installed: {error}"
    );
}

#[tokio::test]
async fn dropping_the_host_stops_it_serving() {
    let network = LocalBridgeNetwork::new();
    let peer = network.bind("peer").unwrap();
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>,
        "host",
        options(env_with_only_claude()),
        replying_executor("still here"),
    )
    .unwrap();

    drop(host);

    // Dropping the host aborts its drain loop, so a frame sent afterwards is
    // never picked up — no ack, no reply, nothing. Asserted on behaviour rather
    // than on the endpoint still being bound: the send closure holds its own
    // clone of the bridge, so the address may outlive the host that served it.
    let _ = peer.send("host", &task_frame("t3", "anyone home?")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut received = Received::new(peer);
    received.pump().await;
    assert!(!received.seen(TaskFrameKind::Ack));
    assert!(!received.seen(TaskFrameKind::Reply));
}

#[tokio::test]
async fn an_explicitly_named_default_provider_is_honoured_when_it_is_installed() {
    // The rejection path is covered above; this is the accepting half, which is
    // what an operator with several CLIs installed actually exercises.
    let network = LocalBridgeNetwork::new();
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>,
        "host",
        EmbeddedDaemonOptions {
            default_provider: Some(HarnessProvider::Claude),
            ..options(env_with_only_claude())
        },
        replying_executor("unused"),
    )
    .expect("claude is installed, so naming it is valid");

    assert_eq!(host.default_provider(), HarnessProvider::Claude);
    assert_eq!(
        host.observation().default_provider(),
        HarnessProvider::Claude
    );
}

#[tokio::test]
async fn an_explicit_workspace_is_resolved_rather_than_the_launch_directory() {
    // The workspace decides what a peer's task can edit, so "the directory I
    // named" and "the directory I happened to launch from" must not be confused.
    let dir = std::env::temp_dir();
    let network = LocalBridgeNetwork::new();
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(network.bind("host").unwrap()) as Arc<dyn Bridge>,
        "host",
        EmbeddedDaemonOptions {
            workspace: dir.to_string_lossy().into_owned(),
            ..options(env_with_only_claude())
        },
        replying_executor("unused"),
    )
    .unwrap();

    let resolved = std::fs::canonicalize(&dir).unwrap_or(dir);
    assert_eq!(host.workspace(), resolved.to_string_lossy());
    // The runtime is reachable for a caller that needs to look past the wrapper.
    assert_eq!(host.runtime().clone().shutdown(), ());
}

#[tokio::test]
async fn a_host_describes_itself_by_where_it_is_and_what_it_serves() {
    // `EmbeddedDaemon` has no derivable Debug — the runtime and join handle have
    // no useful representation — so the hand-written one is worth pinning.
    let (host, _peer) = hosted(replying_executor("unused"));

    let rendered = format!("{host:?}");

    assert!(rendered.contains("EmbeddedDaemon"), "got: {rendered}");
    assert!(
        rendered.contains("host"),
        "should name its address: {rendered}"
    );
    assert!(
        rendered.contains("Claude"),
        "should name what it serves: {rendered}"
    );
}

#[test]
fn the_primary_workspace_always_leads_the_advertised_directories() {
    // An orchestrator reading the list top-down should see where tasks actually
    // run before the alternatives.
    let dirs = accessible_dirs("/srv/primary", &[]);
    assert_eq!(dirs, vec!["/srv/primary".to_string()]);
}

#[test]
fn extra_workspaces_are_appended_without_duplicating_the_primary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let primary = tmp.path().join("primary");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let primary = std::fs::canonicalize(&primary).unwrap();
    let other = std::fs::canonicalize(&other).unwrap();

    let dirs = accessible_dirs(
        &primary.to_string_lossy(),
        &[
            other.to_string_lossy().into_owned(),
            // The primary again, and a repeat of `other`: a list naming one
            // directory twice reads as two places to work.
            primary.to_string_lossy().into_owned(),
            other.to_string_lossy().into_owned(),
        ],
    );
    assert_eq!(
        dirs,
        vec![
            primary.to_string_lossy().into_owned(),
            other.to_string_lossy().into_owned()
        ]
    );
}

#[test]
fn a_blank_entry_does_not_advertise_the_launch_directory() {
    // `resolve_workspace("")` yields the process cwd. A stray blank line in the
    // config must not silently expose whatever folder medulla was started in.
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let dirs = accessible_dirs("/srv/primary", &["".to_string(), "   ".to_string()]);
    assert_eq!(dirs, vec!["/srv/primary".to_string()]);
    assert!(!dirs.contains(&cwd), "{dirs:?}");
}
