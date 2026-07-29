//! Tests for the workflow MCP server.
//!
//! These check the two things a model depends on and cannot recover from: that
//! the protocol handshake is well-formed, and that a tool's *failure* comes back
//! as readable content rather than a protocol error the client swallows.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::harness_contract::is_reserved_tool_name;
use crate::workflows::{FileWorkflowStore, WorkflowStore};

fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Sweep",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

/// The host configuration these tests run against: permissive defaults, which
/// is what a machine with no `[workflows]` section has.
fn config() -> crate::config::WorkflowsConfig {
    crate::config::WorkflowsConfig::default()
}

/// Call a tool and return its parsed result payload plus the error flag.
async fn call(store: &Arc<dyn WorkflowStore>, name: &str, arguments: Value) -> (Value, bool) {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let response = handle_request(store, &config(), &request)
        .await
        .expect("a response");
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().expect("text content");
    (
        serde_json::from_str(text).expect("tool results are JSON"),
        result["isError"].as_bool().unwrap_or(false),
    )
}

#[tokio::test]
async fn initialize_answers_with_the_protocol_version_and_tool_capability() {
    let (_root, store) = store();
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
    assert!(
        response["result"]["capabilities"]["tools"].is_object(),
        "a client decides whether to call tools/list from this: {response}"
    );
}

#[tokio::test]
async fn a_notification_gets_no_reply() {
    // JSON-RPC: a request without an id is a notification. Answering one would
    // desynchronise the client's request correlation.
    let (_root, store) = store();
    let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });

    assert!(handle_request(&store, &config(), &notification)
        .await
        .is_none());
}

#[tokio::test]
async fn tools_list_returns_every_tool_with_a_schema_and_a_description() {
    let (_root, store) = store();
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");
    let tools = response["result"]["tools"].as_array().expect("a tool list");

    assert_eq!(tools.len(), TOOL_NAMES.len());
    for tool in tools {
        let name = tool["name"].as_str().expect("a name");
        assert!(TOOL_NAMES.contains(&name), "unexpected tool {name}");
        assert!(
            tool["description"].as_str().is_some_and(|d| d.len() > 40),
            "{name} needs a description a model can act on"
        );
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
    }
}

#[test]
fn no_tool_name_collides_with_a_harness_reserved_name() {
    // A collision throws when the harness composes its own modules, so this is
    // a build-time-ish guard on a runtime failure.
    for name in TOOL_NAMES {
        assert!(!is_reserved_tool_name(name), "{name} is reserved");
    }
}

#[test]
fn the_catalog_description_is_generated_from_the_node_contracts() {
    // Generated rather than written, so it cannot go stale when a node kind is
    // added or renamed upstream.
    let catalog = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "workflow_catalog")
        .expect("the catalog tool");
    let description = catalog["description"].as_str().unwrap();

    for kind in crate::workflows::node_contracts::NODE_KINDS {
        assert!(
            description.contains(kind),
            "{kind} missing from: {description}"
        );
    }
}

#[tokio::test]
async fn creating_then_listing_a_workflow_works_over_the_tool_surface() {
    let (_root, store) = store();

    let (created, is_error) = call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;
    assert!(!is_error, "{created}");
    assert_eq!(created["created"], "sweep");

    let (listed, _) = call(&store, "workflow_list", json!({})).await;
    assert_eq!(listed["workflows"][0]["id"], "sweep");
}

#[tokio::test]
async fn a_tool_failure_comes_back_as_readable_content_not_a_protocol_error() {
    // The distinction that matters: a protocol error is the *client's* problem
    // and the model may never see it; content with isError is something the
    // model reads and can correct.
    let (_root, store) = store();

    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_get", "arguments": { "id": "ghost" } },
    });
    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    assert!(
        response.get("error").is_none(),
        "a missing workflow is the model's problem to fix, not a broken call: {response}"
    );
    assert_eq!(response["result"]["isError"], true);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("ghost"), "{text}");
}

#[tokio::test]
async fn a_missing_required_argument_is_a_protocol_error_naming_the_argument() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_get", "arguments": {} },
    });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("id"),
        "{response}"
    );
}

#[tokio::test]
async fn validate_accepts_either_a_saved_id_or_an_unsaved_document() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    let (saved, _) = call(&store, "workflow_validate", json!({ "id": "sweep" })).await;
    assert_eq!(saved["ok"], true);

    let (inline, _) = call(
        &store,
        "workflow_validate",
        json!({ "document": document("draft") }),
    )
    .await;
    assert_eq!(inline["ok"], true);

    // Neither handle is a usage error worth naming.
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_validate", "arguments": {} },
    });
    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("document"));
}

#[tokio::test]
async fn validation_failures_are_reported_as_an_answer_rather_than_an_error() {
    let (_root, store) = store();
    let broken = json!({
        "id": "broken",
        "nodes": [{ "id": "a", "kind": "transform", "name": "a" }],
        "edges": []
    })
    .to_string();

    let (result, is_error) = call(&store, "workflow_validate", json!({ "document": broken })).await;

    assert!(!is_error, "asking is not failing: {result}");
    assert_eq!(result["ok"], false);
    assert!(!result["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn applying_ops_edits_the_saved_workflow() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    let (updated, is_error) = call(
        &store,
        "workflow_apply_ops",
        json!({
            "id": "sweep",
            "ops": [{ "op": "set_node_name", "id": "work", "name": "Renamed" }]
        }),
    )
    .await;

    assert!(!is_error, "{updated}");
    assert_eq!(updated["workflow"]["graph"]["nodes"][1]["name"], "Renamed");
}

#[tokio::test]
async fn a_dry_run_is_available_over_the_tool_surface() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    let (result, is_error) = call(&store, "workflow_dry_run", json!({ "id": "sweep" })).await;

    assert!(!is_error, "{result}");
    assert_eq!(result["ok"], true);
    assert!(result["output"]["nodes"]["work"].is_object(), "{result}");
}

#[tokio::test]
async fn an_unknown_tool_lists_the_ones_that_exist() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_teleport", "arguments": {} },
    });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("workflow_list"), "{message}");
}

#[tokio::test]
async fn an_unknown_method_is_a_method_not_found() {
    let (_root, store) = store();
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "sampling/createMessage" });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32601);
}

#[tokio::test]
async fn resource_and_prompt_probes_are_answered_with_empty_lists() {
    // A client probing these before deciding what the server offers should not
    // conclude the server is broken.
    let (_root, store) = store();
    for method in ["resources/list", "prompts/list"] {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method });
        let response = handle_request(&store, &config(), &request)
            .await
            .expect("a response");
        assert!(response.get("error").is_none(), "{method}: {response}");
    }
}

#[tokio::test]
async fn initialize_answers_with_the_version_the_client_asked_for_when_it_is_one_we_speak() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-03-26" },
    });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    // Answering with a fixed version regardless is the one reply that tells a
    // client nothing: it looks identical whether or not its request landed.
    assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
}

#[tokio::test]
async fn initialize_falls_back_to_our_own_version_for_one_we_do_not_speak() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "1999-01-01" },
    });

    let response = handle_request(&store, &config(), &request)
        .await
        .expect("a response");

    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
}

#[tokio::test]
async fn every_advertised_tool_is_one_the_dispatch_actually_handles() {
    let (_root, store) = store();

    for definition in tool_definitions() {
        let name = definition["name"].as_str().expect("a name");
        assert!(
            TOOL_NAMES.contains(&name),
            "{name} is advertised but not in TOOL_NAMES"
        );
        // Called with no arguments, so most answer "missing required argument"
        // — which is fine. What must never come back is "unknown tool": a tool
        // listed but not dispatched reads to a model as its own mistake, and it
        // will keep trying.
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": {} },
        });
        let response = handle_request(&store, &config(), &request)
            .await
            .expect("a response");
        let message = response["error"]["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("unknown tool"),
            "{name} is advertised but not dispatched: {response}"
        );
    }
}

#[tokio::test]
async fn the_host_tool_reports_what_this_machine_permits() {
    let (_root, store) = store();

    let (facts, is_error) = call(&store, "workflow_host", json!({})).await;

    assert!(!is_error, "{facts}");
    // The native slugs are the ones always available; an author guessing at
    // them is how a graph saves cleanly and fails at run time.
    assert!(facts["nativeTools"]
        .as_array()
        .unwrap()
        .contains(&json!("medulla:echo")));
    assert!(facts["allowCode"].is_boolean());
    assert!(
        facts["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("manual")),
        "the trigger limitation has to be stated: {facts}"
    );
}

#[tokio::test]
async fn the_host_tool_says_plainly_when_there_is_no_default_worker() {
    let (_root, store) = store();

    let (facts, _) = call(&store, "workflow_host", json!({})).await;

    // Default config configures none, so every agent node must name one — and
    // a node that does not fails at run time rather than at save time.
    assert_eq!(facts["defaultWorker"], json!(null));
    assert!(
        facts["notes"][0]
            .as_str()
            .unwrap()
            .contains("must name a `config.agent_ref`"),
        "{facts}"
    );
}

#[tokio::test]
async fn history_and_delete_are_reachable_over_the_tool_surface() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;
    call(
        &store,
        "workflow_apply_ops",
        json!({ "id": "sweep",
                "ops": [{ "op": "set_node_name", "id": "work", "name": "Renamed" }] }),
    )
    .await;

    let (history, is_error) = call(&store, "workflow_history", json!({ "id": "sweep" })).await;
    assert!(!is_error, "{history}");
    assert_eq!(history["revisions"].as_array().unwrap().len(), 1);

    let (deleted, is_error) = call(&store, "workflow_delete", json!({ "id": "sweep" })).await;
    assert!(!is_error, "{deleted}");
    assert_eq!(deleted["deleted"], json!("sweep"));
}

#[tokio::test]
async fn there_is_no_tool_that_starts_real_work() {
    // An authoring turn that can run a workflow has stopped being an authoring
    // turn: the operator runs it from the pane they are looking at, where they
    // can see what it does.
    for forbidden in ["workflow_run", "workflow_cancel", "workflow_cancel_run"] {
        assert!(
            !TOOL_NAMES.contains(&forbidden),
            "{forbidden} must not be on the authoring belt"
        );
    }
}
