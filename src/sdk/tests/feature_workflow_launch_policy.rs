//! (Unix-only: spawns a `/bin/sh` fake harness to read the argv it was given.)
//!
//! The launch policy — commit attribution and the operator's `[[hooks]]` —
//! reaching a harness that a *workflow* launched.
//!
//! Every other spawn door was covered: the pty pane, the wrapper, the headless
//! daemon, the executor. A workflow was not, and it is the door with the least
//! supervision behind it — nobody is watching an `agent` node run, which is
//! exactly when a checkpoint hook or a `Co-authored-by` trailer matters most.
//! What the gap looked like from outside: a hook declared in `config.toml` fired
//! under `medulla claude` and under the TUI's own pane, and silently did nothing
//! for the same repository's workflow runs.
//!
//! So this drives the real seam — [`LocalRun`] onto a real embedded host,
//! dispatching a real `agent` node to a real spawned process — and reads the
//! argv that process was actually launched with.

#![cfg(all(unix, feature = "workflows"))]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use medulla::harness_hooks::{HooksConfig, LaunchPolicy};
use medulla::workflows::local::LocalRun;
use medulla::workflows::ops;
use medulla::workflows::{FileWorkflowStore, WorkflowStore};

mod support;

use support::fake_provider::TempDir;

/// A fake `claude` that records its own argv and then answers like the real one.
///
/// The recording is the whole point: a hook is delivered as a flag, so "did the
/// hook reach the harness" is answerable only by reading what the process was
/// started with.
fn recording_claude(dir: &TempDir, log: &std::path::Path) -> String {
    dir.write_script(
        "claude",
        &format!(
            "#!/bin/sh\n\
             for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{log}'; done\n\
             printf '%s\\n' '{{\"type\":\"result\",\"result\":\"done\"}}'\n",
            log = log.display(),
        ),
    )
}

/// A store on a scratch root, with its temporary directory kept alive.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("a temp dir");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A workflow of one `agent` node, so the run reaches the harness spawn seam.
fn save_an_agent_workflow(store: &Arc<dyn WorkflowStore>, id: &str) {
    let document = json!({
        "id": id,
        "name": "Work",
        "description": "dispatches one agent step",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it", "harness": "claude" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    ops::create(store, &document, id).expect("saves the workflow");
}

/// One `PostToolUse` hook, as an operator's config would resolve to.
fn one_hook() -> HooksConfig {
    serde_json::from_value(json!([
        {
            "event": "PostToolUse",
            "matcher": "Edit|Write",
            "type": "command",
            "command": "just checkpoint",
        }
    ]))
    .expect("the hook parses")
}

/// Run `id` to completion under `launch`, and answer with the harness argv.
async fn argv_of_a_run(launch: &LaunchPolicy) -> Vec<String> {
    let (root, store) = store();
    save_an_agent_workflow(&store, "work");

    let dir = TempDir::new();
    let log = std::path::Path::new(&dir.path_str()).join("argv.log");
    let bin = recording_claude(&dir, &log);

    // Only `claude` exists, and it is the recording script. `HOME` keeps the
    // run's own state off the developer's machine.
    let env = HashMap::from([
        ("PATH".to_string(), String::new()),
        ("MEDULLA_CLAUDE_BIN".to_string(), bin),
        (
            "HOME".to_string(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "MEDULLA_HOME".to_string(),
            root.path().join("medulla").to_string_lossy().into_owned(),
        ),
    ]);

    let config = medulla::config::WorkflowsConfig::default();
    let started = LocalRun {
        store: store.clone(),
        config: &config,
        custom_harnesses: &[],
        launch,
        env: &env,
        cwd: root.path(),
        workspace: None,
        workflow_id: "work",
        input: tinyflows::engine::RunInput::new(json!({})),
        sink: None,
        liveness: None,
        origin: None,
    }
    .start()
    .await
    .expect("the run starts");
    started
        .settled()
        .await
        .expect("the workflow run settles successfully");

    std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The JSON document `claude --settings` was handed, if it was handed one.
fn settings_document(argv: &[String]) -> Option<Value> {
    let at = argv.iter().position(|arg| arg == "--settings")?;
    serde_json::from_str(argv.get(at + 1)?).ok()
}

#[tokio::test]
async fn a_workflows_harness_is_launched_with_the_operators_hooks() {
    let argv = argv_of_a_run(&LaunchPolicy {
        attribution: false,
        hooks: one_hook(),
    })
    .await;

    let settings = settings_document(&argv).unwrap_or_else(|| {
        panic!("the workflow's harness was launched with no --settings: {argv:?}")
    });
    let command = settings
        .pointer("/hooks/PostToolUse/0/hooks/0/command")
        .and_then(Value::as_str);
    assert_eq!(
        command,
        Some("just checkpoint"),
        "the operator's hook must reach a harness a workflow launched: {settings}",
    );
}

#[tokio::test]
async fn a_workflows_harness_is_launched_with_commit_attribution() {
    let argv = argv_of_a_run(&LaunchPolicy {
        attribution: true,
        hooks: HooksConfig::default(),
    })
    .await;

    let settings = settings_document(&argv).unwrap_or_else(|| {
        panic!("the workflow's harness was launched with no --settings: {argv:?}")
    });
    assert_eq!(
        settings
            .pointer("/attribution/commit")
            .and_then(Value::as_str),
        Some(medulla::attribution::attribution_trailer().as_str()),
        "a workflow's commits must be attributed like every other Medulla spawn",
    );
}

#[tokio::test]
async fn a_workflows_harness_keeps_hooks_and_commit_attribution_together() {
    let argv = argv_of_a_run(&LaunchPolicy {
        attribution: true,
        hooks: one_hook(),
    })
    .await;

    let settings = settings_document(&argv).unwrap_or_else(|| {
        panic!("the workflow's harness was launched with no --settings: {argv:?}")
    });
    assert_eq!(
        settings
            .pointer("/hooks/PostToolUse/0/hooks/0/command")
            .and_then(Value::as_str),
        Some("just checkpoint"),
        "hooks must not suppress commit attribution: {settings}",
    );
    assert_eq!(
        settings
            .pointer("/attribution/commit")
            .and_then(Value::as_str),
        Some(medulla::attribution::attribution_trailer().as_str()),
        "commit attribution must not suppress hooks: {settings}",
    );
}

#[tokio::test]
async fn an_opted_out_workflow_run_carries_neither() {
    let argv = argv_of_a_run(&LaunchPolicy::default()).await;

    assert!(
        settings_document(&argv).is_none(),
        "an operator who declared no hooks and opted out of attribution gets \
         neither: {argv:?}",
    );
}
