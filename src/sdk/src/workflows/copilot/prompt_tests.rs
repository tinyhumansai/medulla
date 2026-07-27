//! Tests for the copilot prompt.

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

#[test]
fn the_prompt_names_the_one_workflow_the_turn_may_touch() {
    let prompt = build("sweep", "Nightly sweep", &graph(1), "add a step");

    assert!(prompt.contains("id: sweep"), "{prompt}");
    assert!(prompt.contains("name: Nightly sweep"));
    assert!(
        prompt.to_lowercase().contains("edit only the workflow"),
        "the scope has to be stated, not implied"
    );
}

#[test]
fn the_prompt_teaches_the_tools_rather_than_file_editing() {
    let prompt = build("sweep", "Sweep", &graph(1), "go");

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
    assert!(
        prompt.contains("Do not write workflow files directly"),
        "the store is layered, so direct file edits are the wrong path"
    );
}

#[test]
fn every_tool_the_prompt_names_is_one_the_server_actually_serves() {
    let prompt = build("sweep", "Sweep", &graph(1), "go");
    let served = crate::workflows::mcp::TOOL_NAMES;

    for word in prompt.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if word.starts_with("workflow_") {
            assert!(served.contains(&word), "{word} is not a served tool");
        }
    }
}

#[test]
fn the_instruction_is_passed_through_unaltered_and_comes_last() {
    let prompt = build("sweep", "Sweep", &graph(1), "  add a Slack step  ");

    assert!(prompt.trim_end().ends_with("add a Slack step"), "{prompt}");
}

#[test]
fn a_small_graph_is_pasted_in_whole() {
    let prompt = build("sweep", "Sweep", &graph(2), "go");

    assert!(prompt.contains("```json"));
    assert!(prompt.contains("\"n0\""));
}

#[test]
fn a_large_graph_is_outlined_and_the_agent_is_told_to_fetch_it() {
    let prompt = build("sweep", "Sweep", &graph(400), "go");

    assert!(!prompt.contains("```json"), "a huge graph is not pasted");
    assert!(prompt.contains("Call `workflow_get`"));
    assert!(prompt.contains("- n0 (agent) — n0 step"));
    assert!(prompt.contains("- n399 (agent)"));
}

#[test]
fn the_node_and_edge_counts_are_not_described_in_the_plural_when_there_is_one() {
    let prompt = build("sweep", "Sweep", &graph(1), "go");

    assert!(prompt.contains("1 node, 0 edges"), "{prompt}");
}

#[test]
fn an_unnamed_node_is_outlined_without_a_dangling_dash() {
    let mut big = graph(400);
    big.nodes[0].name = "  ".into();

    let prompt = build("sweep", "Sweep", &big, "go");

    assert!(prompt.contains("- n0 (agent)\n"), "{prompt}");
}
