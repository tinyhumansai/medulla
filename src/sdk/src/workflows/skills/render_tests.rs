//! Unit tests for skill and command rendering: slugging, frontmatter shape,
//! description phrasing, and the input table/example JSON built from a
//! workflow's declared inputs.
//!
//! Kept apart from [`super::tests`], which exercises the install/sync/marker
//! pipeline that consumes this rendered text rather than the text itself.

use serde_json::json;

use crate::workflows::{InputType, WorkflowInput, WorkflowSummary};

use super::*;

/// A listing view with the fields skill rendering actually reads.
fn summary(id: &str, description: &str, inputs: Vec<WorkflowInput>) -> WorkflowSummary {
    WorkflowSummary {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
        enabled: true,
        node_count: 3,
        trigger_kind: Some("manual".to_string()),
        inputs,
    }
}

#[test]
fn slug_prefixes_and_sanitises() {
    assert_eq!(slug_for("babysit"), "medulla-babysit");
    assert_eq!(slug_for("Review PR"), "medulla-review-pr");
    assert_eq!(slug_for("a//b"), "medulla-a-b");
    assert_eq!(slug_for("!!!"), "medulla-workflow");
}

#[test]
fn zero_input_skill_states_an_empty_inputs_object() {
    let skill = render(&summary("babysit", "Watch a pull request.", vec![]));

    assert_eq!(skill.slug, "medulla-babysit");
    // The frontmatter opens the file and the marker is the first thing inside
    // it. A harness only parses frontmatter that starts on line 1: with the
    // marker above it, the description — the one field a request is matched
    // against — is not read at all.
    assert!(
        skill
            .body
            .starts_with("---\n# medulla:managed workflow=babysit rev="),
        "{}",
        skill.body
    );
    assert!(skill.body.contains("name: medulla-babysit"));
    assert!(skill.body.contains("This workflow takes no inputs"));
    assert!(skill.body.contains("mcp__medulla__workflow_run"));
    assert!(skill.body.contains("\"inputs\": {}"));
    // The fallback keeps the skill useful with no server attached.
    assert!(skill
        .body
        .contains("medulla workflow run babysit --inputs '{}'"));
    assert!(skill.body.contains("medulla skills install --with-mcp"));
    // The asynchrony is stated, because it is the surprising part: a model that
    // assumed the call waits would report a started run as a finished one.
    assert!(skill.body.contains("does not wait for the run"));
    assert!(skill.body.contains("mcp__medulla__workflow_run_get"));
}

#[test]
fn description_carries_the_workflow_words_and_a_trigger_clause() {
    let skill = render(&summary("babysit", "Watch a pull request", vec![]));
    assert_eq!(
        skill.description,
        "Watch a pull request. Use when the operator asks to run the Medulla \
         \"babysit\" workflow, or describes the work it does."
    );
}

#[test]
fn description_is_non_empty_when_the_workflow_has_none() {
    let skill = render(&summary("babysit", "   ", vec![]));
    assert_eq!(
        skill.description,
        "Use when the operator asks to run the Medulla \"babysit\" workflow, or \
         describes the work it does."
    );
    // Frontmatter carries it as a double-quoted scalar, so the workflow name's
    // own quotes are escaped rather than ending the string early.
    assert!(skill.body.contains(
        "description: \"Use when the operator asks to run the Medulla \\\"babysit\\\" workflow, \
         or describes the work it does.\"\n"
    ));
}

#[test]
fn optional_inputs_render_their_defaults_in_the_example() {
    let inputs = vec![
        WorkflowInput::new("repo", InputType::String)
            .with_default(json!("current"))
            .with_description("Repository to inspect"),
        WorkflowInput::new("deep", InputType::Boolean),
    ];
    let skill = render(&summary("audit", "Audit a repo.", inputs));

    assert!(skill
        .body
        .contains("| `repo` | string | no | Repository to inspect | `\"current\"` |"));
    assert!(skill.body.contains("| `deep` | boolean | no | — | — |"));
    // The default is shown as a runnable value; the undeclared one as a
    // type-shaped placeholder.
    assert!(skill.body.contains("\"repo\": \"current\""));
    assert!(skill.body.contains("\"deep\": false"));
}

#[test]
fn required_inputs_are_marked_and_placeheld() {
    let inputs = vec![
        WorkflowInput::new("pr", InputType::Number)
            .required()
            .with_description("The pull request number"),
        WorkflowInput::new("payload", InputType::Json).required(),
    ];
    let skill = render(&summary("babysit", "Watch a PR.", inputs.clone()));

    assert!(skill
        .body
        .contains("| `pr` | number | yes | The pull request number | — |"));
    assert!(skill.body.contains("| `payload` | json | yes | — | — |"));
    assert!(skill.body.contains("\"pr\": 0"));
    assert!(skill.body.contains("do not invent"));

    let command = render_command(&skill, &summary("babysit", "Watch a PR.", inputs));
    assert!(command.contains("argument-hint: \"<pr> <payload>\""));
    assert!(command.contains("$ARGUMENTS"));
    // A command file is frontmatter-led too, so its marker sits in the same
    // place for the same reason.
    assert!(
        command.starts_with("---\n# medulla:managed workflow=babysit rev="),
        "{command}"
    );
}

#[test]
fn rev_changes_only_when_the_body_does() {
    let first = render(&summary("babysit", "Watch a PR.", vec![]));
    let same = render(&summary("babysit", "Watch a PR.", vec![]));
    let changed = render(&summary("babysit", "Watch a PR closely.", vec![]));

    assert_eq!(first.rev, same.rev);
    assert_eq!(first.body, same.body);
    assert_ne!(first.rev, changed.rev);

    // The rev fingerprints the generated content, and appears only on the
    // marker line itself — so reading it back off the file cannot pick up a
    // hash the content happens to quote.
    assert!(first.body.contains(&format!("rev={}", first.rev)));
    let elsewhere: String = first
        .body
        .lines()
        .filter(|line| !line.starts_with("# medulla:managed "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!elsewhere.contains(&first.rev));
}
