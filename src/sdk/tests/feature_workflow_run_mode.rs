#![cfg(feature = "workflows")]

//! The trigger-only MCP surface, driven as a harness drives it.
//!
//! This is what an operator's own Claude Code or Codex session gets once a
//! generated skill is installed: the real request handler, over a real store,
//! in `MEDULLA_WORKFLOW_TOOLS=run` mode. The unit tests in
//! `src/sdk/src/mcp/tests/run_mode.rs` check the allow-list against the tool
//! table; this checks the two things a skill actually depends on — that
//! `tools/list` shows exactly the six read/run verbs, and that `workflow_run`
//! comes back with a run record.
//!
//! Offline and deterministic: the store is a tempdir, the workflow is one
//! `transform` node so no coding-agent process is ever spawned, and provider
//! detection is pinned through the documented `TINYPLACE_CLAUDE_BIN` override
//! to this test binary's own path so the embedded host starts on a machine with
//! no agent CLI installed.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use medulla::mcp::{handle_request, McpSession, ToolMode};
use medulla::workflows::{ops, FileWorkflowStore, WorkflowStore};
use tokio::sync::Mutex;

/// Exactly what a trigger-only session may see and call.
const TRIGGER_VERBS: [&str; 6] = [
    "workflow_list",
    "workflow_get",
    "workflow_dry_run",
    "workflow_run",
    "workflow_runs",
    "workflow_run_get",
];

/// A store rooted in a scratch directory; the tempdir is returned so the caller
/// keeps it alive.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("a scratch store root");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A trigger-only session over `store`, as `medulla mcp` builds one when the
/// environment asks for run mode.
fn session(store: &Arc<dyn WorkflowStore>) -> McpSession {
    McpSession::local(
        store.clone(),
        medulla::workflows::ops::HostPolicy::default(),
        ToolMode::from_wire(Some("run")),
    )
}

/// A workflow that finishes without dispatching to a harness.
///
/// `transform` evaluates its expressions in process, which is what makes a real
/// `workflow_run` in a test both fast and repeatable: everything the engine does
/// here is arithmetic on the trigger payload.
fn arithmetic_workflow(id: &str) -> String {
    json!({
        "id": id,
        "name": "Double",
        "description": "Doubles the number it is given",
        "inputs": [
            { "name": "count", "type": "number", "required": true,
              "description": "The number to double" }
        ],
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "double", "kind": "transform", "name": "Double",
              "config": { "set": { "doubled": "=inputs.count * 2" } } }
        ],
        "edges": [{ "from_node": "t", "to_node": "double" }]
    })
    .to_string()
}

/// Serialises the tests in this file.
///
/// One of them has to write process environment variables (see
/// [`pin_process_env`]), and `set_var` racing another thread's read of the same
/// table is not something a test should gamble on. Cargo gives this file its own
/// process, so holding this for the whole of each test makes the binary
/// single-threaded without slowing anything else down.
///
/// Async rather than `std::sync`, because each test holds it across the awaits
/// of its tool calls.
static SERIAL: Mutex<()> = Mutex::const_new(());

/// Pin the environment `workflow_run` reads off the process.
///
/// The `workflow_run` arm builds its `env` and `cwd` from the real process
/// rather than from injected values, so a hermetic run has to say what those
/// are: a scratch `MEDULLA_HOME` so the run never reads the developer's own,
/// and a `TINYPLACE_*_BIN` override so provider detection succeeds without a
/// coding-agent CLI installed. The override points at this test binary, which is
/// never executed — the workflow has no `agent` node to dispatch.
fn pin_process_env(home: &Path) {
    let bin = std::env::current_exe().expect("the test binary has a path");
    std::env::set_var("TINYPLACE_CLAUDE_BIN", bin);
    std::env::set_var("MEDULLA_HOME", home);
}

/// Call one tool and return `(payload, isError)`.
async fn call(session: &McpSession, name: &str, arguments: Value) -> (Value, bool) {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let response = handle_request(session, &request)
        .await
        .expect("a request gets a reply");
    let result = &response["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{name} answered without text content: {response}"));
    (
        serde_json::from_str(text).expect("tool results are JSON"),
        result["isError"].as_bool().unwrap_or(false),
    )
}

#[tokio::test]
async fn a_trigger_only_session_lists_exactly_the_six_read_and_run_verbs() {
    let _serial = SERIAL.lock().await;
    let (_root, store) = store();
    ops::create(&store, &arithmetic_workflow("double"), "double").expect("installs");
    let session = session(&store);

    let response = handle_request(
        &session,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await
    .expect("a reply");

    let listed: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(listed, TRIGGER_VERBS.to_vec());

    // And the withheld half is refused at the call, not just hidden from the
    // list — a model that learned `workflow_delete` elsewhere still cannot use
    // it against the operator's own store.
    let (refusal, is_error) = call(&session, "workflow_delete", json!({ "id": "double" })).await;
    assert!(is_error, "{refusal}");
    assert!(store.get("double").expect("store reads").is_some());
}

#[tokio::test]
async fn workflow_run_answers_a_trigger_only_session_with_a_finished_run_record() {
    let _serial = SERIAL.lock().await;
    let home = tempfile::tempdir().expect("a scratch home");
    pin_process_env(home.path());
    let (_root, store) = store();
    ops::create(&store, &arithmetic_workflow("double"), "double").expect("installs");
    let session = session(&store);

    let (result, is_error) = call(
        &session,
        "workflow_run",
        json!({ "id": "double", "inputs": { "count": 21 } }),
    )
    .await;

    assert!(!is_error, "{result}");
    assert_eq!(result["ok"], json!(true), "{result}");
    let run = &result["run"];
    assert_eq!(run["workflowId"], json!("double"), "{result}");
    assert_eq!(run["status"], json!("succeeded"), "{result}");
    let run_id = run["id"].as_str().expect("a run id").to_string();

    // The record is not just returned, it is recorded: the two reading verbs a
    // skill falls back on find the same run.
    let (runs, is_error) = call(&session, "workflow_runs", json!({ "id": "double" })).await;
    assert!(!is_error, "{runs}");
    assert_eq!(runs["runs"][0]["id"], json!(run_id), "{runs}");

    let (fetched, is_error) = call(&session, "workflow_run_get", json!({ "runId": run_id })).await;
    assert!(!is_error, "{fetched}");
    // `workflow_run_get` answers with the record itself, unwrapped.
    assert_eq!(fetched["id"], json!(run_id), "{fetched}");
    assert_eq!(fetched["status"], json!("succeeded"), "{fetched}");
    assert_eq!(
        fetched["steps"][0]["output"][0]["json"]["doubled"],
        json!(42),
        "the declared input reached the graph: {fetched}"
    );
}
