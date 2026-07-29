//! Tool availability and enforcement for restricted evolution turns.

use super::*;

#[test]
fn propose_mode_withholds_every_verb_that_writes_a_graph() {
    for withheld in [
        "workflow_create",
        "workflow_apply_ops",
        "workflow_delete",
        "workflow_run",
    ] {
        assert!(!ToolMode::Propose.allows(withheld));
        assert!(ToolMode::Full.allows(withheld));
    }
}

#[test]
fn propose_mode_keeps_everything_needed_to_read_reason_and_propose() {
    for kept in [
        "workflow_list",
        "workflow_get",
        "workflow_host",
        "workflow_catalog",
        "workflow_preview_ops",
        "workflow_validate",
        "workflow_dry_run",
        "workflow_runs",
        "workflow_run_get",
        "workflow_history",
        "workflow_notes",
        "workflow_note_add",
        "workflow_proposals",
        "workflow_propose",
    ] {
        assert!(ToolMode::Propose.allows(kept), "{kept} is needed to review");
    }
}

#[test]
fn a_review_turn_is_not_shown_the_tools_it_may_not_call() {
    let listed: Vec<String> = tool_definitions(ToolMode::Propose)
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    assert!(!listed.iter().any(|name| name == "workflow_apply_ops"));
    assert!(listed.iter().any(|name| name == "workflow_propose"));
    assert_eq!(listed.len(), TOOL_NAMES.len() - 4);
}

#[tokio::test]
async fn a_review_turn_calling_a_withheld_tool_is_refused_and_told_what_to_use() {
    let (_root, store) = store();
    crate::workflows::ops::create(&store, &document("sweep"), "sweep").expect("installs");
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_apply_ops", "arguments": { "id": "sweep", "ops": [] } }
    });
    let response = handle_request(&store, &config(), ToolMode::Propose, &request)
        .await
        .expect("a request gets a reply");
    let result = &response["result"];
    assert_eq!(result["isError"], json!(true));
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("workflow_propose"), "{text}");
}

#[test]
fn the_mode_is_read_from_the_environment_and_defaults_to_full() {
    use std::collections::HashMap;

    assert_eq!(ToolMode::from_env(&HashMap::new()), ToolMode::Full);
    assert_eq!(
        ToolMode::from_env(&HashMap::from([(
            TOOL_MODE_ENV.to_string(),
            "propose".to_string()
        )])),
        ToolMode::Propose
    );
    assert_eq!(
        ToolMode::from_env(&HashMap::from([(
            TOOL_MODE_ENV.to_string(),
            "nonsense".to_string()
        )])),
        ToolMode::Full
    );
}

#[test]
fn review_writes_are_scoped_to_the_reviewed_workflow() {
    let foreign = crate::workflows::mcp::tools::scope_error_for(
        ToolMode::Propose,
        "workflow_note_add",
        &json!({ "id": "other" }),
        Some("sweep"),
    );
    assert!(foreign.is_some_and(|error| error.contains("sweep")));
    assert!(crate::workflows::mcp::tools::scope_error_for(
        ToolMode::Propose,
        "workflow_propose",
        &json!({ "id": "sweep" }),
        Some("sweep"),
    )
    .is_none());
}
