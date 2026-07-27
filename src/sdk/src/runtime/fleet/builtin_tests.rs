//! Unit tests for the built-in agent-template catalog: the shape every default
//! role must have, and the additive merge that lets a declared template of the
//! same id replace one.

use super::builtin::{builtin_templates, with_builtin_templates};
use super::{AgentTemplate, CapacitySnapshot};

/// A minimal declared template, so the merge has something to collide with.
fn declared(id: &str, description: &str) -> AgentTemplate {
    AgentTemplate {
        id: id.into(),
        name: None,
        description: description.into(),
        instructions: None,
        tools: None,
        model: None,
        effort: None,
        params: Default::default(),
        tags: Vec::new(),
        metadata: Default::default(),
        harnesses: Default::default(),
    }
}

#[test]
fn every_builtin_is_declared_completely_and_uniquely() {
    let templates = builtin_templates();
    assert!(templates.len() >= 8, "a useful starting catalog");

    let mut ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
    ids.sort_unstable();
    let unique = {
        let mut u = ids.clone();
        u.dedup();
        u
    };
    assert_eq!(ids, unique, "ids collide, so a merge would drop one");

    for template in &templates {
        assert!(template.name.is_some(), "{} has no name", template.id);
        assert!(
            !template.description.trim().is_empty(),
            "{} has no description",
            template.id
        );
        // The description is the catalog line and the instructions are the
        // prompt surface: a role missing either is a row you cannot act on.
        assert!(
            template
                .instructions
                .as_deref()
                .is_some_and(|i| !i.trim().is_empty()),
            "{} has no instructions",
            template.id
        );
        assert!(
            template.tools.as_ref().is_some_and(|t| !t.is_empty()),
            "{} grants no tools",
            template.id
        );
        // An empty override map means "any harness kind"; a non-empty one is an
        // allowlist, which a default role must never impose.
        assert!(
            template.harnesses.is_empty(),
            "{} restricts harness kinds",
            template.id
        );
        assert!(
            template.tags.contains(&"code".to_string()),
            "{} is not tagged as coding work",
            template.id
        );
    }
}

#[test]
fn builtins_fill_an_empty_catalog() {
    let out = with_builtin_templates(&CapacitySnapshot::default());
    assert_eq!(out.templates.len(), builtin_templates().len());
    // Only the catalog is populated — the containment chain stays as declared.
    assert!(out.hosts.is_empty() && out.harnesses.is_empty() && out.workspaces.is_empty());
}

#[test]
fn a_declared_template_outranks_the_builtin_of_the_same_id() {
    let capacity = CapacitySnapshot {
        templates: vec![declared("code-reviewer", "ours, not theirs")],
        ..Default::default()
    };
    let out = with_builtin_templates(&capacity);

    let reviewers: Vec<&AgentTemplate> = out
        .templates
        .iter()
        .filter(|t| t.id == "code-reviewer")
        .collect();
    assert_eq!(reviewers.len(), 1, "one row per id");
    assert_eq!(reviewers[0].description, "ours, not theirs");
    // The declared catalog reads first; the rest of the built-ins still arrive.
    assert_eq!(out.templates[0].id, "code-reviewer");
    assert_eq!(out.templates.len(), builtin_templates().len());
}

#[test]
fn merging_is_idempotent() {
    let once = with_builtin_templates(&CapacitySnapshot::default());
    let twice = with_builtin_templates(&once);
    assert_eq!(once, twice);
}
