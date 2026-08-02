//! Tests for correlating private graph tags with durable prompt evidence.

use serde_json::json;

use crate::workflows::RunStep;

use super::{instrumented, AgentEvidence, NODE_ID_FIELD};

#[test]
fn only_agent_nodes_receive_private_evidence_tags() {
    let graph = serde_json::from_value(json!({
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start", "config": {} },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "=item.task" } }
        ],
        "edges": [{ "from_node": "start", "to_node": "work" }]
    }))
    .unwrap();

    let tagged = instrumented(&graph);

    assert!(graph.nodes[1].config.get(NODE_ID_FIELD).is_none());
    assert_eq!(tagged.nodes[1].config[NODE_ID_FIELD], json!("work"));
    assert!(tagged.nodes[0].config.get(NODE_ID_FIELD).is_none());
}

#[test]
fn every_prompt_from_a_per_item_agent_is_kept() {
    let evidence = AgentEvidence::default();
    evidence.record(&json!({ (NODE_ID_FIELD): "work" }), "Review the first file");
    evidence.record(
        &json!({ (NODE_ID_FIELD): "work" }),
        "Review the second file",
    );
    let mut steps = [RunStep {
        node_id: "work".into(),
        status: "success".into(),
        duration_ms: 4,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    }];

    evidence.attach(&mut steps);

    assert_eq!(
        steps[0].input,
        Some(json!(["Review the first file", "Review the second file"]))
    );
}

#[test]
fn prompt_queue_is_bounded_before_the_run_finishes() {
    let evidence = AgentEvidence::default();
    let request = json!({ (NODE_ID_FIELD): "work" });
    evidence.record(&request, &"x".repeat(128 * 1024));
    evidence.record(&request, "must not grow the queue again");
    let mut steps = [RunStep {
        node_id: "work".into(),
        status: "success".into(),
        duration_ms: 4,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    }];

    evidence.attach(&mut steps);

    let input = serde_json::to_vec(&steps[0].input).unwrap();
    assert!(input.len() <= 64 * 1024 + 256);
    assert_eq!(steps[0].input.as_ref().unwrap()["_medullaTruncated"], true);
}
