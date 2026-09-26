#![cfg(feature = "workflows")]

//! The trigger-only MCP surface, driven as a harness drives it.
//!
//! This is what an operator's own Claude Code or Codex session gets once a
//! generated skill is installed: the real request handler, over a real store,
//! in `MEDULLA_WORKFLOW_TOOLS=run` mode. The unit tests in
//! `src/sdk/src/mcp/tests/run_mode.rs` check the allow-list against the tool
//! table; this checks the two things a skill actually depends on — that
//! `tools/list` shows exactly the read/run verbs, and that `workflow_run`
//! reaches a run record, whether the skill follows the run id it is handed or
//! asks to wait for the whole thing.
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
const TRIGGER_VERBS: [&str; 8] = [
    "workflow_list",
    "workflow_get",
    "workflow_dry_run",
    "workflow_run",
    "workflow_runs",
    "workflow_run_get",
    "workflow_run_detail",
    "workflow_run_cancel",
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
///
/// It also *removes* the control-plane grant, which is the other half of reading
/// the real process environment. A test binary is routinely launched inside a
/// harness session Medulla itself started — `cargo test` run by a coding agent
/// under the Sessions tab — and inherits that session's socket and token.
/// [`medulla::workflows::RunReporter`] reports to whatever grant the run's
/// environment names, so without this the fixture below files five runs of a
/// workflow called `double` into the *operator's* live rail, where they sit
/// wedged at `running` forever because this process exits without ever sending a
/// terminal report. With no socket and no token `grant_from_env` returns `None`,
/// no reporter is started, and the run behaves exactly as it does under
/// `medulla workflow run` in an ordinary shell.
///
/// The parent-handoff pair goes too: those are what a *nested* session exchanges
/// for a child capability, so leaving them would reopen the same door one level
/// down.
fn pin_process_env(home: &Path) {
    let bin = std::env::current_exe().expect("the test binary has a path");
    std::env::set_var("TINYPLACE_CLAUDE_BIN", bin);
    std::env::set_var("MEDULLA_HOME", home);
    for key in [
        medulla::control_socket::MCP_SOCKET_ENV,
        medulla::control_socket::MCP_GRANT_ENV,
        medulla::control_socket::HOOK_SOCKET_ENV,
        medulla::control_socket::HOOK_GRANT_ENV,
        "MEDULLA_MCP_PARENT_SOCKET",
        "MEDULLA_MCP_PARENT_GRANT",
    ] {
        std::env::remove_var(key);
    }
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
async fn a_trigger_only_session_lists_exactly_the_read_and_run_verbs() {
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

/// Poll `run_id` through the session until it settles.
///
/// What a skill does with the run id it is handed. Bounded so a run that never
/// settles fails the test rather than hanging it.
async fn settled(session: &McpSession, run_id: &str) -> Value {
    for _ in 0..200 {
        let (run, is_error) = call(session, "workflow_run_get", json!({ "runId": run_id })).await;
        assert!(!is_error, "{run}");
        if run["status"] != json!("running") {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("run '{run_id}' never settled");
}

#[tokio::test]
async fn workflow_run_hands_a_trigger_only_session_a_run_id_it_can_follow() {
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

    // The default is async, because a skill triggers real workflows and a real
    // workflow outlives the harness's idle ceiling. `ok` here says the run
    // started, and the answer says so in words as well.
    assert!(!is_error, "{result}");
    assert_eq!(result["ok"], json!(true), "{result}");
    assert_eq!(result["status"], json!("running"), "{result}");
    assert_eq!(result["workflowId"], json!("double"), "{result}");
    let run_id = result["runId"].as_str().expect("a run id").to_string();

    // The run is recorded from the moment it is admitted, so the two reading
    // verbs a skill falls back on find it immediately.
    let (runs, is_error) = call(&session, "workflow_runs", json!({ "id": "double" })).await;
    assert!(!is_error, "{runs}");
    assert_eq!(runs["runs"][0]["id"], json!(run_id), "{runs}");

    let fetched = settled(&session, &run_id).await;
    assert_eq!(fetched["id"], json!(run_id), "{fetched}");
    assert_eq!(fetched["status"], json!("succeeded"), "{fetched}");
    assert_eq!(
        fetched["steps"][0]["output"][0]["json"]["doubled"],
        json!(42),
        "the declared input reached the graph: {fetched}"
    );
}

#[tokio::test]
async fn workflow_run_still_answers_with_the_whole_record_when_asked_to_wait() {
    let _serial = SERIAL.lock().await;
    let home = tempfile::tempdir().expect("a scratch home");
    pin_process_env(home.path());
    let (_root, store) = store();
    ops::create(&store, &arithmetic_workflow("double"), "double").expect("installs");
    let session = session(&store);

    // The opt-in, for a workflow this small: arithmetic on a trigger payload
    // settles long before any client would give up on it.
    let (result, is_error) = call(
        &session,
        "workflow_run",
        json!({ "id": "double", "inputs": { "count": 21 }, "wait": true }),
    )
    .await;

    assert!(!is_error, "{result}");
    assert_eq!(result["ok"], json!(true), "{result}");
    let run = &result["run"];
    assert_eq!(run["workflowId"], json!("double"), "{result}");
    assert_eq!(run["status"], json!("succeeded"), "{result}");
    assert_eq!(
        run["steps"][0]["output"][0]["json"]["doubled"],
        json!(42),
        "the declared input reached the graph: {result}"
    );
}

#[tokio::test]
async fn a_trigger_only_session_inspects_its_run_and_cancels_it_after_it_settled() {
    let _serial = SERIAL.lock().await;
    let home = tempfile::tempdir().expect("a scratch home");
    pin_process_env(home.path());
    let (_root, store) = store();
    ops::create(&store, &arithmetic_workflow("double"), "double").expect("installs");
    let session = session(&store);

    let (started, is_error) = call(
        &session,
        "workflow_run",
        json!({ "id": "double", "inputs": { "count": 21 } }),
    )
    .await;
    assert!(!is_error, "{started}");
    let run_id = started["runId"].as_str().expect("a run id").to_string();
    let settled = settled(&session, &run_id).await;
    assert_eq!(settled["status"], json!("succeeded"), "{settled}");

    // The in-depth read is the same record plus the live half beside it. This
    // session has no fleet behind it, so the live half says so rather than
    // reporting an empty roster as "nothing is running".
    let (detail, is_error) =
        call(&session, "workflow_run_detail", json!({ "runId": run_id })).await;
    assert!(!is_error, "{detail}");
    assert_eq!(detail["run"]["id"], json!(run_id), "{detail}");
    assert_eq!(
        detail["live"]["taskIdPrefix"],
        json!(format!("wf:{run_id}:")),
        "{detail}"
    );
    assert!(detail["live"]["fleetUnavailable"].is_string(), "{detail}");

    // Cancelling a run that already finished is an answer, not a failure: the
    // caller wanted it stopped and it is stopped. The other half — a cancel
    // that lands on a run genuinely mid-harness-session — needs a dispatch that
    // hangs on demand, so it is exercised where that stand-in lives, in
    // `workflows::run::tests::cases`.
    let (cancelled, is_error) =
        call(&session, "workflow_run_cancel", json!({ "runId": run_id })).await;
    assert!(!is_error, "{cancelled}");
    assert_eq!(cancelled["cancelled"], json!(false), "{cancelled}");
    assert_eq!(cancelled["runId"], json!(run_id), "{cancelled}");
}

/// A fixture run must never reach the operator's own control plane.
///
/// `workflow_run` reads its environment off the process, and this binary is
/// routinely launched from a harness session Medulla started — so it inherits a
/// live socket and token unless something takes them away. When it did, these
/// tests filed runs of a workflow named `double` into the operator's Agents
/// rail, wedged at `running` because the process exits without a terminal
/// report. Asserting on `grant_from_env` rather than on the reporter is
/// deliberate: that call is the single gate the reporter is built behind, so a
/// `None` here is the guarantee that no report can be sent at all.
#[tokio::test]
async fn a_run_started_in_this_binary_reports_to_no_control_plane() {
    let _serial = SERIAL.lock().await;
    let home = tempfile::tempdir().expect("a scratch home");

    // Exactly what a harness session hands its children.
    std::env::set_var(
        medulla::control_socket::MCP_SOCKET_ENV,
        home.path().join("control.sock"),
    );
    std::env::set_var(medulla::control_socket::MCP_GRANT_ENV, "a-live-token");

    pin_process_env(home.path());

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    assert!(
        medulla::control_socket::grant_from_env(&env).is_none(),
        "an inherited grant survived pin_process_env: these tests would report \
         their runs into the operator's live rail"
    );
}
