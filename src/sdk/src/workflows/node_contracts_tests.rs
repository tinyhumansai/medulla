//! Tests for the host overlay on the node-kind catalogue.

use super::{
    all_node_kind_contracts, apply_host_overlay, node_kind_contract, render_node_kinds_line,
    NODE_KINDS,
};

#[test]
fn every_engine_node_kind_is_still_present_after_the_overlay() {
    let kinds: Vec<String> = all_node_kind_contracts()
        .into_iter()
        .map(|contract| contract.kind)
        .collect();

    assert_eq!(kinds.len(), NODE_KINDS.len());
    for kind in NODE_KINDS {
        assert!(kinds.iter().any(|k| k == kind), "{kind} went missing");
    }
}

#[test]
fn the_overlay_only_adds_notes_and_never_edits_what_the_engine_said() {
    let original = tinyflows::catalog::contract_for("agent").unwrap();
    let overlaid = apply_host_overlay(original.clone());

    assert_eq!(overlaid.kind, original.kind);
    assert_eq!(overlaid.summary, original.summary);
    assert_eq!(overlaid.description, original.description);
    assert_eq!(overlaid.config_fields, original.config_fields);
    assert_eq!(overlaid.example, original.example);
    assert!(
        overlaid.notes.len() > original.notes.len(),
        "host notes should have been appended"
    );
    assert!(
        original
            .notes
            .iter()
            .all(|note| overlaid.notes.contains(note)),
        "the engine's own notes must survive"
    );
}

#[test]
fn the_agent_contract_explains_that_a_node_is_a_harness_session() {
    // The single most surprising thing about workflows on this host, so it must
    // be in the document an author actually reads.
    let contract = node_kind_contract("agent").unwrap();
    let notes = contract.notes.join(" ");

    assert!(notes.contains("harness"), "notes were: {notes}");
    assert!(notes.contains("agent_ref"));
}

#[test]
fn the_tool_call_contract_lists_the_native_tools_it_actually_has() {
    let contract = node_kind_contract("tool_call").unwrap();
    let notes = contract.notes.join(" ");

    for tool in crate::flow_engine::caps::tools::NATIVE_TOOLS {
        assert!(notes.contains(tool), "{tool} should be listed: {notes}");
    }
}

#[test]
fn the_code_contract_warns_that_the_host_has_no_sandbox() {
    let notes = node_kind_contract("code").unwrap().notes.join(" ");

    assert!(notes.contains("sandbox"), "notes were: {notes}");
}

#[test]
fn an_unknown_node_kind_has_no_contract() {
    assert!(node_kind_contract("teleport").is_none());
}

/// Guards against a tool description drifting from the catalogue: the prose is
/// generated, so it cannot go stale when a node kind is added or renamed.
#[test]
fn the_rendered_summary_covers_every_kind_one_line_each() {
    let rendered = render_node_kinds_line();
    let lines: Vec<&str> = rendered.lines().collect();

    assert_eq!(lines.len(), NODE_KINDS.len());
    for kind in NODE_KINDS {
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(&format!("{kind}:"))),
            "{kind} missing from: {rendered}"
        );
    }
}
