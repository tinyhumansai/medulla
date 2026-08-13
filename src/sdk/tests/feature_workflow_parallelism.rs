//! The engine's parallelism primitives, running on a real worker.
//!
//! `scatter`/`gather` and `spawn`/`gate` are only meaningful on this host if
//! each lane and each spawned task reaches a *harness*. That is the join the
//! engine cannot make on its own: it opens the lanes, and Medulla is what turns
//! a lane's `agent` node into a dispatched coding session. A graph that opened
//! its lanes and then ran one harness, or none, would still complete and still
//! report success — which is exactly why these assert on the prompts the
//! executor was actually asked to run rather than on the reply text.
//!
//! Offline and process-free, like the sibling dispatch suite: the executor is
//! substituted so no coding agent is ever spawned.

#![cfg(feature = "workflows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::bridge::{Bridge, LocalBridge, LocalBridgeNetwork};
use medulla::daemon::embedded::{EmbeddedDaemon, EmbeddedDaemonOptions};
use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::protocol::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrame, TaskFrameKind,
};
use medulla::workflows::{FileWorkflowStore, WorkflowStore};
use serde_json::json;

/// A path guaranteed to exist and be executable on every platform: this test
/// binary. `/bin/sh` is not present on Windows.
fn installed_bin() -> String {
    std::env::current_exe()
        .expect("the test binary has a path")
        .to_string_lossy()
        .into_owned()
}

/// An environment in which exactly `claude` is "installed", with `MEDULLA_HOME`
/// pointed at a scratch directory so the worker's workflow store is the one this
/// test wrote and not the developer's own.
fn env_in(home: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), String::new()),
        ("MEDULLA_CLAUDE_BIN".to_string(), installed_bin()),
        (
            "MEDULLA_HOME".to_string(),
            home.to_string_lossy().into_owned(),
        ),
    ])
}

/// An executor that records every prompt it was asked to run.
///
/// The recording is the whole assertion surface here: one entry per harness
/// session, so a fan-out that silently collapsed to a single session is
/// visible.
fn recording_executor(seen: Arc<Mutex<Vec<String>>>) -> RunTaskFn {
    Arc::new(move |options: RunTaskOptions| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.lock().unwrap().push(options.prompt.clone());
            Ok(RunTaskResult {
                provider: options.provider,
                reply: format!("ran: {}", options.prompt),
                events: 1,
                usage: None,
                session_id: None,
            })
        })
    })
}

/// Install `document` as workflow `id` in the worker's store.
fn install(home: &std::path::Path, id: &str, document: serde_json::Value) {
    // `MEDULLA_HOME` names the root that holds accounts; the worker resolves its
    // store from the account directory inside it, and nobody signs in here.
    let home = home.join("local");
    let store = FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    );
    let record = medulla::workflows::store::parse_workflow(&document.to_string(), id)
        .expect("valid fixture");
    store.save(&record).expect("installs");
}

/// Fingerprint the exact installed record a simulated orchestrator selected.
fn installed_fingerprint(home: &std::path::Path, id: &str) -> String {
    let home = home.join("local");
    let store = FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    );
    let record = store.get(id).unwrap().expect("installed workflow");
    medulla::workflows::record_fingerprint(&record)
}

/// Start a worker whose home is `home`, returning it with a peer endpoint.
fn worker(home: &std::path::Path, run_task: RunTaskFn) -> (EmbeddedDaemon, LocalBridge) {
    let network = LocalBridgeNetwork::new();
    let host_bridge = network.bind("host").unwrap();
    let peer = network.bind("peer").unwrap();
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(host_bridge) as Arc<dyn Bridge>,
        "host",
        EmbeddedDaemonOptions {
            env: env_in(home),
            workspace: home.to_string_lossy().into_owned(),
            poll: Duration::from_millis(5),
            ..Default::default()
        },
        run_task,
    )
    .expect("worker starts with claude detected");
    (host, peer)
}

/// A frame naming an installed workflow.
fn frame(task_id: &str, workflow: &str, fingerprint: &str) -> String {
    encode_task_frame(EncodeFrameInput {
        transport: None,
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: String::new(),
        ts: "T".to_string(),
        correlation_id: Some(format!("corr-{task_id}")),
        harness: None,
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: Some(workflow.to_string()),
        workflow_fingerprint: Some(fingerprint.to_string()),
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    })
}

/// The frames drained from a peer so far.
///
/// The buffer has to outlive one wait, because `drain_inbox` is destructive: an
/// `Ack` and the `Reply` that follows it commonly land in the same drain window.
#[derive(Default)]
struct Inbox {
    seen: Vec<TaskFrame>,
}

impl Inbox {
    /// Drain the peer's inbox until a frame of `kind` shows up.
    async fn wait_for(&mut self, peer: &LocalBridge, kind: TaskFrameKind) -> TaskFrame {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            for message in peer.drain_inbox(50).await {
                if let Some(frame) = decode_task_frame(&message.text) {
                    self.seen.push(frame);
                }
            }
            let found = self.seen.iter().position(|frame| frame.kind == kind);
            if let Some(index) = found {
                return self.seen.remove(index);
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no {kind:?} frame within the deadline; saw {:?}",
                self.seen
                    .iter()
                    .map(|f| (f.kind, f.text.clone()))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// Run installed workflow `id` on a fresh worker and return the prompts its
/// harness sessions were asked to run.
///
/// `task_id` must differ per test: the worker's in-flight registry is
/// process-global, and the test binary runs its cases concurrently in one
/// process, so a shared id makes the second frame bounce with "already
/// running".
async fn prompts_from_running(home: &std::path::Path, id: &str, task_id: &str) -> Vec<String> {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (_host, peer) = worker(home, recording_executor(prompts.clone()));
    let mut inbox = Inbox::default();

    let fingerprint = installed_fingerprint(home, id);
    peer.send("host", &frame(task_id, id, &fingerprint))
        .await
        .unwrap();

    inbox.wait_for(&peer, TaskFrameKind::Ack).await;
    let reply = inbox.wait_for(&peer, TaskFrameKind::Reply).await;
    assert!(
        !reply.text.to_lowercase().contains("failed"),
        "the run should have completed: {}",
        reply.text
    );

    let seen = prompts.lock().unwrap().clone();
    seen
}

/// A `scatter` opens one lane per item and every lane runs the whole body, so a
/// three-item scatter over a one-agent body is three harness sessions — not one
/// session handed three items.
#[tokio::test]
async fn a_scatter_runs_the_lane_body_once_per_lane_on_a_real_harness() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "fan-out",
        json!({
            "id": "fan-out",
            "name": "Fan out over three repos",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "seed", "kind": "transform", "name": "Seed the list",
                  "config": { "set": { "repos": "=[\"alpha\", \"beta\", \"gamma\"]" } } },
                { "id": "fan", "kind": "scatter", "name": "One lane per repo",
                  "config": { "path": "repos" } },
                { "id": "work", "kind": "agent", "name": "Review",
                  "config": { "prompt": "review one repo" } },
                { "id": "collect", "kind": "gather", "name": "Collect the lanes",
                  "config": { "from": ["work"], "release": "all" } }
            ],
            "edges": [
                { "from_node": "t", "to_node": "seed" },
                { "from_node": "seed", "to_node": "fan" },
                { "from_node": "fan", "to_node": "work" },
                { "from_node": "work", "to_node": "collect" }
            ]
        }),
    );

    let prompts = prompts_from_running(home.path(), "fan-out").await;
    assert_eq!(
        prompts.len(),
        3,
        "one harness session per lane; got {prompts:?}"
    );
}

/// A `spawn` targeting a child workflow must actually run that child's `agent`
/// node on a harness. The engine's own default task runner settles the ticket
/// with an echo of the spec and dispatches nothing, so an empty recording here
/// is precisely the regression this guards.
#[tokio::test]
async fn a_spawned_child_workflow_dispatches_its_agent_node_to_a_harness() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "spawn-and-gate",
        json!({
            "id": "spawn-and-gate",
            "name": "Start work, collect it later",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "kick", "kind": "spawn", "name": "Start the background job",
                  "config": {
                      "target": "workflow",
                      "workflow": {
                          "id": "child",
                          "name": "child",
                          "nodes": [
                              { "id": "ct", "kind": "trigger", "name": "start",
                                "config": { "trigger_kind": "manual" } },
                              { "id": "cwork", "kind": "agent", "name": "Background work",
                                "config": { "prompt": "the spawned prompt" } }
                          ],
                          "edges": [{ "from_node": "ct", "to_node": "cwork" }]
                      }
                  } },
                { "id": "collect", "kind": "gate", "name": "Collect it",
                  "config": { "from": ["kick"], "release": "all", "poll_interval_ms": 25 } }
            ],
            "edges": [
                { "from_node": "t", "to_node": "kick" },
                { "from_node": "kick", "to_node": "collect" }
            ]
        }),
    );

    let prompts = prompts_from_running(home.path(), "spawn-and-gate").await;
    assert!(
        prompts.iter().any(|p| p.contains("the spawned prompt")),
        "the spawned child's agent node should have reached a harness; got {prompts:?}"
    );
}
