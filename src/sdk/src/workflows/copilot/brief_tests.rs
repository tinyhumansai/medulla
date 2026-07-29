//! Tests for the rendered turn brief.
//!
//! Two kinds of assertion here, and the second matters more. The first is that
//! a mode says what it should — a repair turn names the run, a create turn does
//! not name an existing workflow. The second is that the standing rules cannot
//! drift from the code they describe: a prompt that teaches a tool the server
//! does not serve is a prompt that wastes a round trip on every turn.

use super::*;
use serde_json::json;
use tinyflows::model::{Node, NodeKind};

fn node(id: &str) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Agent,
        type_version: 1,
        name: format!("{id} step"),
        config: json!({}),
        ports: Vec::new(),
        position: None,
    }
}

fn graph(nodes: usize) -> WorkflowGraph {
    WorkflowGraph {
        nodes: (0..nodes).map(|index| node(&format!("n{index}"))).collect(),
        ..Default::default()
    }
}

fn record(nodes: usize) -> WorkflowRecord {
    WorkflowRecord {
        id: "sweep".into(),
        name: "Nightly sweep".into(),
        description: String::new(),
        enabled: true,
        graph: graph(nodes),
        source_path: None,
    }
}

/// A revise turn, the common case.
fn revise<'a>(record: &'a WorkflowRecord, instruction: &'a str) -> String {
    CopilotRequest {
        mode: Mode::Revise,
        instruction,
        record: Some(record),
        run: None,
    }
    .render()
}

// ---- the standing rules ----

#[test]
fn every_tool_the_prompt_names_is_one_the_server_actually_serves() {
    // The rules are prose and the tool list is code; nothing but this stops
    // them drifting. A prompt that teaches a tool the server does not serve
    // costs a failed call on every turn that believes it.
    let prompt = revise(&record(1), "go");
    let served = crate::workflows::mcp::TOOL_NAMES;

    for word in prompt.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if word.starts_with("workflow_") {
            assert!(served.contains(&word), "{word} is not a served tool");
        }
    }
}

#[test]
fn the_rules_teach_the_tools_rather_than_file_editing() {
    let prompt = revise(&record(1), "go");

    for tool in [
        "workflow_get",
        "workflow_catalog",
        "workflow_preview_ops",
        "workflow_apply_ops",
        "workflow_validate",
        "workflow_dry_run",
    ] {
        assert!(prompt.contains(tool), "{tool} is not taught");
    }
    // The store is layered, so the file an agent would find by searching is not
    // necessarily the record the operator is looking at.
    assert!(prompt.contains("layered"), "{prompt}");
}

#[test]
fn the_rules_say_what_an_agent_node_actually_is_on_this_host() {
    let prompt = revise(&record(1), "go");

    // The single fact most likely to produce a wrong graph if assumed: an
    // agent node here is a harness session, not a model call.
    assert!(prompt.contains("coding harness"), "{prompt}");
    assert!(prompt.contains("agent_ref"), "{prompt}");
}

#[test]
fn the_rules_are_honest_about_which_triggers_fire() {
    let prompt = revise(&record(1), "go");

    // Otherwise the copilot builds "every morning at 9" as a schedule trigger,
    // stores it, and describes it as done — and it never runs.
    assert!(prompt.contains("Only `manual` triggers fire"), "{prompt}");
}

#[test]
fn the_rules_state_a_stop_condition_for_the_verify_loop() {
    let prompt = revise(&record(1), "go");

    assert!(prompt.contains("Stop condition"), "{prompt}");
}

// ---- modes ----

#[test]
fn a_revise_turn_names_the_one_workflow_it_may_touch() {
    let prompt = revise(&record(1), "add a step");

    assert!(prompt.contains("id: sweep"), "{prompt}");
    assert!(prompt.contains("name: Nightly sweep"));
    assert!(prompt.contains("only that one"), "{prompt}");
}

#[test]
fn a_create_turn_names_no_existing_workflow() {
    let prompt = CopilotRequest {
        mode: Mode::Create,
        instruction: "summarise new issues daily",
        record: None,
        run: None,
    }
    .render();

    assert!(prompt.contains("workflow_create"), "{prompt}");
    assert!(prompt.contains("only adds"), "{prompt}");
    // Naming one is how an agent talks itself into editing it.
    assert!(!prompt.contains("id: sweep"), "{prompt}");
}

#[test]
fn a_repair_turn_carries_the_run_the_error_and_the_failing_nodes() {
    let prompt = CopilotRequest {
        mode: Mode::Repair,
        instruction: "why did this fail?",
        record: Some(&record(1)),
        run: Some(FailedRun {
            id: "run-42".into(),
            error: Some("worker refused the task".into()),
            failing_nodes: vec!["notify".into(), "publish".into()],
        }),
    }
    .render();

    // All three are known to the caller already; making the agent rediscover
    // them starts the turn a step behind.
    assert!(prompt.contains("run-42"), "{prompt}");
    assert!(prompt.contains("worker refused the task"), "{prompt}");
    assert!(prompt.contains("notify, publish"), "{prompt}");
}

#[test]
fn a_repair_turn_says_a_cause_the_graph_cannot_fix_is_not_a_graph_edit() {
    let prompt = CopilotRequest {
        mode: Mode::Repair,
        instruction: "fix it",
        record: Some(&record(1)),
        run: Some(FailedRun {
            id: "run-1".into(),
            ..Default::default()
        }),
    }
    .render();

    // Otherwise the agent edits the graph to look busy in the face of a
    // missing harness, and the next run fails identically.
    assert!(prompt.contains("change nothing"), "{prompt}");
}

#[test]
fn a_repair_turn_with_no_recorded_error_still_names_the_run() {
    let prompt = CopilotRequest {
        mode: Mode::Repair,
        instruction: "look into it",
        record: Some(&record(1)),
        run: Some(FailedRun {
            id: "run-7".into(),
            error: None,
            failing_nodes: Vec::new(),
        }),
    }
    .render();

    assert!(prompt.contains("run-7"), "{prompt}");
    assert!(prompt.contains("workflow_runs"), "the record is fetchable");
    // No empty "It failed with:" block for an error that was never recorded.
    assert!(!prompt.contains("It failed with"), "{prompt}");
}

// ---- context ----

#[test]
fn the_instruction_is_passed_through_unaltered_and_comes_last() {
    let prompt = revise(&record(1), "  add a Slack step  ");

    assert!(prompt.trim_end().ends_with("add a Slack step"), "{prompt}");
}

#[test]
fn a_small_graph_is_pasted_in_whole() {
    let prompt = revise(&record(2), "go");

    assert!(prompt.contains("```json"));
    assert!(prompt.contains("\"n0\""));
}

#[test]
fn a_large_graph_is_outlined_and_the_agent_is_told_to_fetch_it() {
    let prompt = revise(&record(400), "go");

    assert!(!prompt.contains("```json"), "a huge graph is not pasted");
    assert!(prompt.contains("Call `workflow_get`"));
    assert!(prompt.contains("- n0 (agent) — n0 step"));
    assert!(prompt.contains("- n399 (agent)"));
}

#[test]
fn an_unnamed_node_is_outlined_without_a_dangling_dash() {
    let mut big = record(400);
    big.graph.nodes[0].name = "  ".into();

    let prompt = revise(&big, "go");

    assert!(prompt.contains("- n0 (agent)\n"), "{prompt}");
}

#[test]
fn the_node_and_edge_counts_are_not_described_in_the_plural_when_there_is_one() {
    let prompt = revise(&record(1), "go");

    assert!(prompt.contains("1 node, 0 edges"), "{prompt}");
}

#[test]
fn a_disabled_workflow_says_so() {
    let mut paused = record(1);
    paused.enabled = false;

    let prompt = revise(&paused, "why is nothing happening?");

    // The agent would otherwise diagnose a graph that is fine and miss that
    // the workflow simply cannot run.
    assert!(prompt.contains("disabled"), "{prompt}");
}

#[test]
fn a_description_is_included_when_there_is_one_and_omitted_when_blank() {
    let mut described = record(1);
    described.description = "sweeps the repo each night".into();

    assert!(revise(&described, "go").contains("sweeps the repo each night"));
    assert!(!revise(&record(1), "go").contains("description:"));
}
