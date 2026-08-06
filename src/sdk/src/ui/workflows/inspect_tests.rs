//! Tests for the node inspector and the run overlay.

use super::*;
use serde_json::json;
use tinyflows::model::{NodeKind, Port};

use crate::workflows::RunStep;

fn node(config: serde_json::Value) -> Node {
    Node {
        id: "step".into(),
        kind: NodeKind::Agent,
        type_version: 1,
        name: "Step".into(),
        config,
        ports: Vec::new(),
        position: None,
    }
}

fn record(steps: Vec<RunStep>, pending: Vec<String>) -> RunRecord {
    RunRecord {
        id: "r1".into(),
        workflow_id: "w".into(),
        status: RunStatus::Running,
        started_at: 0,
        finished_at: None,
        steps,
        pending_approvals: pending,
        error: None,
        inputs: Default::default(),
        trigger: None,
        origin: None,
        summary: None,
        diagnosis: None,
    }
}

fn step(node_id: &str, status: &str) -> RunStep {
    RunStep {
        node_id: node_id.into(),
        status: status.into(),
        duration_ms: 12,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    }
}

#[test]
fn a_node_the_run_never_reached_is_pending_rather_than_missing() {
    let overlay = RunOverlay::new(&record(vec![step("a", "success")], Vec::new()));

    assert_eq!(overlay.node("never-ran").state, NodeRunState::Pending);
    assert_eq!(overlay.reached(), 1);
}

#[test]
fn engine_step_statuses_map_onto_the_states_this_host_draws() {
    let overlay = RunOverlay::new(&record(
        vec![
            step("ok", "success"),
            step("bad", "failed"),
            step("odd", "skipped"),
        ],
        Vec::new(),
    ));

    assert_eq!(overlay.node("ok").state, NodeRunState::Succeeded);
    assert_eq!(overlay.node("bad").state, NodeRunState::Failed);
    assert_eq!(
        overlay.node("odd").state,
        NodeRunState::Other,
        "an unfamiliar status is shown, not guessed at"
    );
}

#[test]
fn a_step_status_is_matched_regardless_of_case_or_padding() {
    let overlay = RunOverlay::new(&record(vec![step("ok", " SUCCESS ")], Vec::new()));

    assert_eq!(overlay.node("ok").state, NodeRunState::Succeeded);
}

#[test]
fn a_gate_the_run_is_parked_on_outranks_its_earlier_step() {
    let overlay = RunOverlay::new(&record(
        vec![step("review", "success")],
        vec!["review".into()],
    ));

    assert_eq!(
        overlay.node("review").state,
        NodeRunState::AwaitingApproval,
        "where the run is beats where it has been"
    );
}

#[test]
fn a_node_that_ran_twice_keeps_the_state_the_run_ended_in() {
    let overlay = RunOverlay::new(&record(
        vec![step("retry", "failed"), step("retry", "success")],
        Vec::new(),
    ));

    assert_eq!(overlay.node("retry").state, NodeRunState::Succeeded);
}

#[test]
fn diagnostics_and_duration_survive_onto_the_overlay() {
    let mut with_diags = step("a", "success");
    with_diags.diagnostics = vec!["=item.missing resolved to null".into()];

    let overlay = RunOverlay::new(&record(vec![with_diags], Vec::new()));

    assert_eq!(overlay.node("a").duration_ms, Some(12));
    assert_eq!(overlay.node("a").diagnostics.len(), 1);
}

#[test]
fn every_state_has_a_word_a_colour_and_a_glyph() {
    for state in [
        NodeRunState::Pending,
        NodeRunState::Succeeded,
        NodeRunState::Failed,
        NodeRunState::AwaitingApproval,
        NodeRunState::Other,
    ] {
        assert!(!state.label().is_empty(), "{state:?}");
        assert!(!state.color().is_empty(), "{state:?}");
        assert!(!state.glyph().is_empty(), "{state:?}");
    }
}

#[test]
fn only_an_unsettled_run_draws_the_graph_as_live() {
    assert!(is_live(RunStatus::Running));
    assert!(is_live(RunStatus::PendingApproval));
    assert!(!is_live(RunStatus::Succeeded));
    assert!(!is_live(RunStatus::Failed));
}

#[test]
fn a_detail_starts_with_the_identity_of_the_node() {
    let rows = node_detail(&node(json!({})));

    assert_eq!(rows[0].label, "id");
    assert_eq!(rows[1].label, "kind");
    assert_eq!(rows[1].value, "agent");
    assert_eq!(rows[2].label, "name");
}

#[test]
fn nested_config_is_flattened_onto_dotted_paths() {
    let rows = node_detail(&node(json!({"retry": {"attempts": 3}, "tags": ["a", "b"]})));

    let found: Vec<(String, String)> = rows
        .iter()
        .map(|row| (row.label.clone(), row.value.clone()))
        .collect();
    assert!(
        found.contains(&("retry.attempts".into(), "3".into())),
        "{found:?}"
    );
    assert!(found.contains(&("tags.0".into(), "a".into())), "{found:?}");
    assert!(found.contains(&("tags.1".into(), "b".into())), "{found:?}");
}

#[test]
fn an_empty_array_still_gets_a_row_because_it_is_often_the_bug() {
    let rows = node_detail(&node(json!({"inputs": []})));

    assert!(rows
        .iter()
        .any(|row| row.label == "inputs" && row.value == "[]"));
}

#[test]
fn a_multi_line_value_continues_under_an_empty_label() {
    let rows = node_detail(&node(json!({"prompt": "first\nsecond"})));

    let start = rows.iter().position(|row| row.label == "prompt").unwrap();
    assert_eq!(rows[start].value, "first");
    assert_eq!(rows[start + 1].label, "");
    assert_eq!(rows[start + 1].value, "second");
}

#[test]
fn a_null_config_value_is_shown_rather_than_dropped() {
    let rows = node_detail(&node(json!({"model": null})));

    assert!(rows
        .iter()
        .any(|row| row.label == "model" && row.value == "null"));
}

#[test]
fn declared_ports_are_listed_with_their_labels() {
    let listed = Node {
        ports: vec![Port {
            name: "true".into(),
            label: Some("matched".into()),
        }],
        ..node(json!({}))
    };

    let rows = node_detail(&listed);

    assert!(rows
        .iter()
        .any(|row| row.label == "port" && row.value == "true — matched"));
}

#[test]
fn a_node_is_found_in_a_graph_by_id() {
    let graph = WorkflowGraph {
        nodes: vec![node(json!({}))],
        ..Default::default()
    };

    assert!(find_node(&graph, "step").is_some());
    assert!(find_node(&graph, "absent").is_none());
}
