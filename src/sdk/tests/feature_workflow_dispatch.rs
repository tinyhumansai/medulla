//! An orchestrator dispatching a *workflow* to a worker, end to end.
//!
//! The claim under test is the one that makes workflows more than a local
//! convenience: a task frame naming an installed workflow makes the worker run
//! that whole graph, dispatching each `agent` node to its own harness, and
//! answer with one reply — over the same transport, admission check, ack, and
//! correlation an ordinary task already uses.
//!
//! Offline and process-free: provider detection is pinned through the documented
//! `TINYPLACE_*_BIN` overrides, and the executor is substituted so no coding
//! agent is ever spawned.

#![cfg(feature = "workflows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::bridge::{Bridge, LocalBridge, LocalBridgeNetwork};
use medulla::daemon::embedded::{EmbeddedDaemon, EmbeddedDaemonOptions};
use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::tinyplace::{
    decode_task_frame, encode_task_frame, parse_agent_capabilities, EncodeFrameInput, TaskFrame,
    TaskFrameKind,
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
        ("TINYPLACE_CLAUDE_BIN".to_string(), installed_bin()),
        (
            "MEDULLA_HOME".to_string(),
            home.to_string_lossy().into_owned(),
        ),
    ])
}

/// An executor that records the prompts it was asked to run and answers each
/// with a fixed reply, so a test can assert on *what the workflow dispatched*.
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

/// Install a two-step workflow into the worker's store.
///
/// The trigger fans out to two agent nodes, so a single dispatched frame has to
/// produce two harness runs for the test to pass.
fn install_workflow(home: &std::path::Path, id: &str) {
    // `MEDULLA_HOME` names the root that holds accounts; the worker resolves its
    // store from the account directory inside it, and nobody signs in here.
    let home = home.join("local");
    let store = FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    );
    let document = json!({
        "id": id,
        "name": "Two step",
        "description": "does two things",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "first", "kind": "agent", "name": "First",
              "config": { "prompt": "step one" } },
            { "id": "second", "kind": "agent", "name": "Second",
              "config": { "prompt": "step two" } }
        ],
        "edges": [
            { "from_node": "t", "to_node": "first" },
            { "from_node": "first", "to_node": "second" }
        ]
    })
    .to_string();
    let record = medulla::workflows::store::parse_workflow(&document, id).expect("valid fixture");
    store.save(&record).expect("installs");
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

/// A frame the orchestrator sends. `workflow` names an installed graph; `None`
/// is an ordinary instruction.
fn frame(task_id: &str, text: &str, workflow: Option<&str>) -> String {
    encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "T".to_string(),
        correlation_id: Some(format!("corr-{task_id}")),
        harness: None,
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: workflow.map(str::to_string),
        conversation: None,
    })
}

/// Drain the peer's inbox until a frame of `kind` shows up.
async fn wait_for(peer: &LocalBridge, kind: TaskFrameKind) -> TaskFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<TaskFrame> = Vec::new();
    loop {
        for message in peer.drain_inbox(50).await {
            if let Some(frame) = decode_task_frame(&message.text) {
                seen.push(frame);
            }
        }
        if let Some(frame) = seen.iter().find(|frame| frame.kind == kind) {
            return frame.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {kind:?} frame within the deadline; saw {:?}",
            seen.iter()
                .map(|f| (f.kind, f.text.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_frame_naming_a_workflow_runs_the_whole_graph_on_the_worker() {
    let home = tempfile::tempdir().unwrap();
    install_workflow(home.path(), "two-step");
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (_host, peer) = worker(home.path(), recording_executor(prompts.clone()));

    peer.send("host", &frame("w1", "", Some("two-step")))
        .await
        .unwrap();

    let ack = wait_for(&peer, TaskFrameKind::Ack).await;
    assert_eq!(ack.task_id, "w1");
    assert_eq!(ack.text, "workflow accepted");

    let reply = wait_for(&peer, TaskFrameKind::Reply).await;
    assert_eq!(reply.correlation_id.as_deref(), Some("corr-w1"));
    assert!(
        reply.text.contains("completed 2 steps"),
        "the reply should summarise the run: {}",
        reply.text
    );

    // One dispatched frame, two harness runs: the graph, not the text, decided
    // what work happened.
    let prompts = prompts.lock().unwrap().clone();
    assert_eq!(
        prompts,
        vec!["step one".to_string(), "step two".to_string()]
    );
}

#[tokio::test]
async fn the_reply_carries_the_run_as_a_work_snapshot_the_orchestrator_can_render() {
    let home = tempfile::tempdir().unwrap();
    install_workflow(home.path(), "two-step");
    let (_host, peer) = worker(
        home.path(),
        recording_executor(Arc::new(Mutex::new(Vec::new()))),
    );

    peer.send("host", &frame("w2", "", Some("two-step")))
        .await
        .unwrap();
    let reply = wait_for(&peer, TaskFrameKind::Reply).await;

    // The same attachment an ordinary task's reply carries, so the existing
    // master-terminal rendering shows a workflow with no new code.
    let work = reply.work.expect("a workflow reply should carry its work");
    assert_eq!(work.todo_progress(), (2, 2), "both steps done: {work:?}");
    assert!(
        work.plan.iter().any(|item| item.text == "First"),
        "the plan should name the graph's steps: {:?}",
        work.plan
    );
}

#[tokio::test]
async fn naming_a_workflow_the_worker_does_not_have_says_what_it_does_have() {
    let home = tempfile::tempdir().unwrap();
    install_workflow(home.path(), "two-step");
    let (_host, peer) = worker(
        home.path(),
        recording_executor(Arc::new(Mutex::new(Vec::new()))),
    );

    peer.send("host", &frame("w3", "", Some("nonexistent")))
        .await
        .unwrap();

    let error = wait_for(&peer, TaskFrameKind::Error).await;
    assert!(
        error.text.contains("nonexistent") && error.text.contains("two-step"),
        "an orchestrator should be able to correct itself from this: {}",
        error.text
    );
}

#[tokio::test]
async fn a_worker_advertises_the_workflows_it_has_installed() {
    let home = tempfile::tempdir().unwrap();
    install_workflow(home.path(), "two-step");
    let (_host, peer) = worker(
        home.path(),
        recording_executor(Arc::new(Mutex::new(Vec::new()))),
    );

    peer.send(
        "host",
        &encode_task_frame(EncodeFrameInput {
            kind: TaskFrameKind::Capabilities,
            task_id: "probe".to_string(),
            text: String::new(),
            ts: "T".to_string(),
            correlation_id: Some("corr-probe".to_string()),
            harness: None,
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            conversation: None,
        }),
    )
    .await
    .unwrap();

    let result = wait_for(&peer, TaskFrameKind::CapabilitiesResult).await;
    let capabilities = parse_agent_capabilities(&result.text).expect("a capabilities payload");

    // This is how the orchestrator learns a worker can do more than take an
    // instruction: the ids here are the ones it may name in a task frame.
    let advertised = capabilities
        .workflows
        .iter()
        .find(|advert| advert.id == "two-step")
        .expect("the installed workflow should be advertised");
    assert_eq!(advertised.name, "Two step");
    assert_eq!(advertised.description, "does two things");
    assert_eq!(advertised.node_count, 3);
}

#[tokio::test]
async fn an_ordinary_instruction_still_goes_straight_to_a_harness() {
    // The workflow route must not change what a frame without one does.
    let home = tempfile::tempdir().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (_host, peer) = worker(home.path(), recording_executor(prompts.clone()));

    peer.send("host", &frame("t1", "just do this", None))
        .await
        .unwrap();
    let reply = wait_for(&peer, TaskFrameKind::Reply).await;

    assert_eq!(reply.text, "ran: just do this");
    assert_eq!(prompts.lock().unwrap().clone(), vec!["just do this"]);
}
