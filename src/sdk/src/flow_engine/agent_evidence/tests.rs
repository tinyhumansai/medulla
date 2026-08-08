//! Tests for correlating private graph tags with durable prompt evidence.

use serde_json::json;

use crate::harness_transcript::TranscriptEntry;
use crate::workflows::RunStep;

use super::{instrumented, AgentEvidence, NODE_ID_FIELD};

/// A step of `node_id` with no evidence yet, as the engine's observer creates
/// them before [`AgentEvidence::attach`] fills the evidence in.
fn step(node_id: &str) -> RunStep {
    RunStep {
        node_id: node_id.into(),
        status: "success".into(),
        duration_ms: 4,
        input: None,
        output: None,
        diagnostics: Vec::new(),
        transcript: Vec::new(),
    }
}

/// One transcript entry of `kind` saying `text`.
fn entry(kind: &str, text: &str) -> TranscriptEntry {
    TranscriptEntry {
        at_ms: 1_700_000_000_000,
        kind: kind.into(),
        text: text.into(),
    }
}

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
        transcript: Vec::new(),
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
        transcript: Vec::new(),
    }];

    evidence.attach(&mut steps);

    let input = serde_json::to_vec(&steps[0].input).unwrap();
    assert!(input.len() <= 64 * 1024 + 256);
    assert_eq!(steps[0].input.as_ref().unwrap()["_medullaTruncated"], true);
}

#[test]
fn an_empty_transcript_keeps_its_slot_in_the_queue() {
    // A looped node where the first activation folded to nothing (only status
    // events, say) and the later ones produced real transcripts. The first
    // activation still occupied a step, so its queue slot must be preserved as
    // an empty placeholder; dropping it would pop the second activation's
    // transcript onto the first step.
    let evidence = AgentEvidence::default();
    evidence.record_transcript("work", Vec::new());
    evidence.record_transcript("work", vec![entry("agent_message", "first pass")]);
    evidence.record_transcript("work", vec![entry("tool_call", "Bash(npm test)")]);
    let mut steps = [step("work"), step("work"), step("work")];

    evidence.attach(&mut steps);

    assert!(
        steps[0].transcript.is_empty(),
        "an empty activation must not claim a later transcript"
    );
    assert_eq!(
        steps[1].transcript,
        vec![entry("agent_message", "first pass")]
    );
    assert_eq!(
        steps[2].transcript,
        vec![entry("tool_call", "Bash(npm test)")]
    );
}

#[test]
fn transcripts_are_queued_in_completion_order_when_none_are_empty() {
    // The all-non-empty control for the mixed test above: each transcript lands
    // on its own step, in order, even when several accumulate before attach.
    let evidence = AgentEvidence::default();
    evidence.record_transcript("work", vec![entry("agent_message", "one")]);
    evidence.record_transcript("work", vec![entry("tool_call", "two")]);
    evidence.record_transcript("work", vec![entry("error", "three")]);
    let mut steps = [step("work"), step("work"), step("work")];

    evidence.attach(&mut steps);

    assert_eq!(steps[0].transcript, vec![entry("agent_message", "one")]);
    assert_eq!(steps[1].transcript, vec![entry("tool_call", "two")]);
    assert_eq!(steps[2].transcript, vec![entry("error", "three")]);
}

#[test]
fn transcripts_from_one_multi_dispatch_activation_fold_onto_its_step() {
    // A per-item activation that dispatched twice queues two transcripts, but
    // the engine records one step for the whole activation. Mirroring the
    // prompt pass, the last step of the node absorbs every queued transcript,
    // so neither is dropped and neither drifts onto a later step.
    let evidence = AgentEvidence::default();
    evidence.record_transcript("work", vec![entry("agent_message", "first file")]);
    evidence.record_transcript("work", vec![entry("tool_call", "Bash(edit)")]);
    let mut steps = [step("work")];

    evidence.attach(&mut steps);

    assert_eq!(
        steps[0].transcript,
        vec![
            entry("agent_message", "first file"),
            entry("tool_call", "Bash(edit)"),
        ]
    );
}

#[test]
fn excess_transcripts_fold_onto_the_last_step_in_completion_order() {
    // More transcripts than steps for a node — a multi-dispatch activation
    // followed by another — stays in completion order: one transcript per step
    // as far as it goes, and the last step absorbs what is left. This is the
    // same grouping the prompt pass uses, so a transcript is never dropped and
    // never reordered, even when the node has only one step to receive a
    // fan-out's worth.
    let evidence = AgentEvidence::default();
    evidence.record_transcript("work", vec![entry("agent_message", "fan one")]);
    evidence.record_transcript("work", vec![entry("agent_message", "fan two")]);
    evidence.record_transcript("work", vec![entry("agent_message", "next activation")]);
    let mut steps = [step("work"), step("work")];

    evidence.attach(&mut steps);

    assert_eq!(steps[0].transcript, vec![entry("agent_message", "fan one")]);
    assert_eq!(
        steps[1].transcript,
        vec![
            entry("agent_message", "fan two"),
            entry("agent_message", "next activation"),
        ]
    );
}
