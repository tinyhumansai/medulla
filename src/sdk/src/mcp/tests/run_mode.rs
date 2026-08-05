//! Tool availability and enforcement for trigger-only sessions.
//!
//! `ToolMode::Run` is what a skill installed into the operator's everyday
//! harness gets, so these check the allow-list from both directions: the six
//! verbs are served, and everything else is absent from `tools/list` *and*
//! refused at `tools/call`.

use super::*;

/// The whole surface a trigger-only session is meant to have.
const TRIGGER_VERBS: [&str; 6] = [
    "workflow_list",
    "workflow_get",
    "workflow_dry_run",
    "workflow_run",
    "workflow_runs",
    "workflow_run_get",
];

#[test]
fn run_mode_serves_exactly_the_six_read_and_trigger_verbs() {
    let served: Vec<&str> = TOOL_NAMES
        .into_iter()
        .filter(|name| ToolMode::Run.allows(name))
        .collect();
    assert_eq!(served, TRIGGER_VERBS.to_vec());
}

#[test]
fn run_mode_withholds_authoring_deletion_and_the_review_surface() {
    for withheld in [
        "workflow_create",
        "workflow_apply_ops",
        "workflow_defaults",
        "workflow_preview_ops",
        "workflow_validate",
        "workflow_delete",
        "workflow_history",
        "workflow_host",
        "workflow_catalog",
        "workflow_notes",
        "workflow_note_add",
        "workflow_proposals",
        "workflow_propose",
    ] {
        assert!(
            !ToolMode::Run.allows(withheld),
            "{withheld} must not reach a trigger-only session"
        );
    }
}

#[test]
fn a_trigger_session_is_not_shown_the_tools_it_may_not_call() {
    let listed: Vec<String> = definitions_for(ToolMode::Run)
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(listed, TRIGGER_VERBS.map(str::to_string).to_vec());
}

#[tokio::test]
async fn a_trigger_session_calling_a_withheld_tool_is_told_to_author_in_medulla() {
    let (_root, store) = store();
    crate::workflows::ops::create(&store, &document("sweep"), "sweep").expect("installs");
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_delete", "arguments": { "id": "sweep" } }
    });
    let response = handle_request(&session(&store, ToolMode::Run), &request)
        .await
        .expect("a request gets a reply");
    let result = &response["result"];
    assert_eq!(result["isError"], json!(true));
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("trigger workflows"), "{text}");
    assert!(text.contains("Medulla"), "{text}");

    // And the workflow really is still there: the refusal happens before
    // dispatch, not as a failed delete.
    assert!(store.get("sweep").expect("store reads").is_some());
}

#[test]
fn the_run_mode_spelling_round_trips_and_unknown_values_stay_restricted() {
    use std::collections::HashMap;

    assert_eq!(ToolMode::Run.as_wire(), "run");
    assert_eq!(
        ToolMode::from_wire(Some(ToolMode::Run.as_wire())),
        ToolMode::Run
    );
    assert_eq!(
        ToolMode::from_env(&HashMap::from([(
            TOOL_MODE_ENV.to_string(),
            "run".to_string()
        )])),
        ToolMode::Run
    );
    // An unrecognised value resolves to the mode that can neither write a graph
    // nor start a run, so a typo can never buy execution.
    assert_eq!(ToolMode::from_wire(Some("trigger")), ToolMode::Propose);
    assert_eq!(
        ToolMode::from_env(&HashMap::from([(
            TOOL_MODE_ENV.to_string(),
            "runner".to_string()
        )])),
        ToolMode::Propose
    );
}

#[test]
fn adding_run_mode_left_full_and_propose_untouched() {
    for name in TOOL_NAMES {
        assert!(ToolMode::Full.allows(name), "{name}");
    }
    let propose: Vec<&str> = TOOL_NAMES
        .into_iter()
        .filter(|name| ToolMode::Propose.allows(name))
        .collect();
    assert_eq!(propose.len(), TOOL_NAMES.len() - 5);
    for withheld in [
        "workflow_create",
        "workflow_apply_ops",
        "workflow_defaults",
        "workflow_delete",
        "workflow_run",
    ] {
        assert!(!ToolMode::Propose.allows(withheld), "{withheld}");
    }
}
