//! Unit tests for the workflow data model.
//!
//! These lean on literal JSON rather than round-tripping Rust values wherever a
//! wire shape is the actual contract: run records and workflow documents are
//! read back from disk by builds other than the one that wrote them, so the
//! spelling of a field is the thing worth asserting.

use serde_json::json;

use super::*;

/// A minimal single-node graph, enough to build a record around.
fn graph() -> tinyflows::model::WorkflowGraph {
    serde_json::from_value(json!({
        "nodes": [{
            "id": "start",
            "kind": "trigger",
            "name": "start",
            "config": { "trigger_kind": "manual" }
        }],
        "edges": []
    }))
    .expect("the fixture graph should parse")
}

fn record() -> WorkflowRecord {
    WorkflowRecord {
        id: "demo".into(),
        name: "Demo".into(),
        description: "A demo workflow".into(),
        enabled: true,
        graph: graph(),
        source_path: None,
    }
}

#[test]
fn summary_counts_nodes_and_reads_the_trigger_kind() {
    let summary = record().summary();

    assert_eq!(summary.id, "demo");
    assert_eq!(summary.node_count, 1);
    assert_eq!(summary.trigger_kind.as_deref(), Some("manual"));
}

#[test]
fn a_document_without_enabled_defaults_to_enabled() {
    let parsed: WorkflowRecord = serde_json::from_value(json!({
        "id": "demo",
        "name": "Demo",
        "graph": graph(),
    }))
    .expect("a document may omit `enabled` and `description`");

    assert!(parsed.enabled);
    assert_eq!(parsed.description, "");
}

#[test]
fn run_status_settles_everything_but_running_and_pending() {
    assert!(!RunStatus::Running.is_settled());
    assert!(!RunStatus::PendingApproval.is_settled());
    for status in [
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert!(status.is_settled(), "{status:?} should be settled");
    }
}

#[test]
fn run_records_use_camel_case_on_the_wire() {
    let wire = serde_json::to_value(RunRecord {
        id: "run-1".into(),
        workflow_id: "demo".into(),
        status: RunStatus::Succeeded,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![RunStep {
            node_id: "start".into(),
            status: "ok".into(),
            duration_ms: 3,
            diagnostics: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
    })
    .expect("a run record should serialize");

    assert!(wire.get("workflowId").is_some());
    assert!(wire.get("startedAt").is_some());
    assert_eq!(wire["status"], json!("succeeded"));
    assert!(
        wire.get("error").is_none(),
        "an absent error should not be written"
    );
    assert!(wire["steps"][0].get("nodeId").is_some());
}
