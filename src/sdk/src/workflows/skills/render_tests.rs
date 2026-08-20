//! Unit tests for skill and command rendering: slugging, frontmatter shape,
//! description phrasing, the input list and example JSON built from a
//! workflow's declared inputs, and the size budget all of that is held to.
//!
//! Kept apart from [`super::tests`], which exercises the install/sync/marker
//! pipeline that consumes this rendered text rather than the text itself.

use serde_json::json;

use crate::workflows::{InputType, WorkflowInput, WorkflowSummary};

use super::render::{condense, MAX_FRONTMATTER_DESCRIPTION_CHARS};
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

/// A workflow documented as thoroughly as the real ones are — the case the size
/// budget has to hold for, since a terse template is easy on a terse workflow.
fn verbose_summary() -> WorkflowSummary {
    summary(
        "pr-babysitter",
        "Babysit a pull request in any checked-out repository: infer the repo from its git \
         remotes, then loop - read CI, review threads, review verdicts and issue comments; fix \
         and reply; attempt the merge - until every comment and review is resolved and the PR is \
         merged, or the iteration cap is hit.",
        vec![
            WorkflowInput::new("pr", InputType::Number)
                .required()
                .with_description("The pull request number."),
            WorkflowInput::new("workdir", InputType::String)
                .with_default(json!("."))
                .with_description(
                    "Directory of the git checkout to work in, relative to the workspace root. \
                     The GitHub repository is inferred from its remotes.",
                ),
            WorkflowInput::new("max_iterations", InputType::Number)
                .with_default(json!(8))
                .with_description(
                    "How many babysit-and-merge passes before giving up (the loop's own hard cap \
                     is 25). A run also has a wall-clock limit on this host and each pass costs a \
                     whole harness session, so this is a ceiling rather than a promise - in \
                     practice a run gets a handful of passes, not this many.",
                ),
            WorkflowInput::new("ci_wait_secs", InputType::Number)
                .with_default(json!(1200))
                .with_description(
                    "How long a pass will wait for in-flight CI checks to settle before giving up \
                     on them. This is a ceiling on a watch, not a fixed sleep: the wait ends as \
                     soon as every check reaches a terminal state, so set it above the slowest \
                     job in the repository's suite rather than to a typical duration.",
                ),
        ],
    )
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
    assert!(skill.body.contains("No inputs — pass `\"inputs\": {}`."));
    assert!(skill.body.contains("mcp__medulla__workflow_run"));
    assert!(skill.body.contains("{\"id\":\"babysit\",\"inputs\":{}}"));
    // The fallback keeps the skill useful with no server attached.
    assert!(skill.body.contains("medulla workflow run babysit --inputs"));
    assert!(skill.body.contains("medulla skills install --with-mcp"));
    // What `workflow_run`'s own MCP description already says — that it answers
    // at once with a runId, that the run outlives the call, that
    // `workflow_run_get` reads it back — is not restated here. The model holds
    // that description whenever it holds the tool, so a copy in the body is a
    // token spent twice.
    assert!(!skill.body.contains("workflow_run_get"), "{}", skill.body);
    assert!(!skill.body.contains("minutes to hours"), "{}", skill.body);
}

#[test]
fn description_carries_the_workflow_words_and_a_trigger_clause() {
    let skill = render(&summary("babysit", "Watch a pull request", vec![]));
    assert_eq!(
        skill.description,
        "Watch a pull request. Medulla workflow \"babysit\" — use when asked to run it, \
         or to do this work."
    );
}

#[test]
fn description_is_non_empty_when_the_workflow_has_none() {
    let skill = render(&summary("babysit", "   ", vec![]));
    assert_eq!(
        skill.description,
        "Medulla workflow \"babysit\" — use when asked to run it, or to do this work."
    );
    // Frontmatter carries it as a double-quoted scalar, so the workflow name's
    // own quotes are escaped rather than ending the string early.
    assert!(skill.body.contains(
        "description: \"Medulla workflow \\\"babysit\\\" — use when asked to run it, or to \
         do this work.\"\n"
    ));
}

#[test]
fn a_long_description_is_condensed_at_a_sentence_boundary() {
    let long = "Survey every open PR across the superproject and its submodules, drop the ones \
                that cannot merge, then review a batch of the eligible ones on a harness. \
                Merging happens only once review passes. Anything ambiguous is left alone for \
                the operator to judge rather than guessed at, because a wrong merge is not \
                something a later pass can undo.";
    let skill = render(&summary("triage", long, vec![]));

    // Cut at a sentence boundary, so the frontmatter still reads as prose.
    assert!(
        skill.description.starts_with(
            "Survey every open PR across the superproject and its submodules, drop the ones that \
             cannot merge, then review a batch of the eligible ones on a harness. Merging happens \
             only once review passes. Medulla workflow \"triage\""
        ),
        "{}",
        skill.description
    );
    assert!(
        skill.description.chars().count() < long.chars().count(),
        "{}",
        skill.description
    );
    // Condensed, never lost: the body says where the sentences the frontmatter
    // dropped still live, rather than reprinting them at the operator's expense.
    assert!(
        skill
            .body
            .contains("`mcp__medulla__workflow_get` has the full description and input notes."),
        "{}",
        skill.body
    );
}

#[test]
fn a_short_description_is_not_repeated_in_the_body() {
    let skill = render(&summary("babysit", "Watch a pull request.", vec![]));
    // The harness has already handed the model these words via the frontmatter;
    // printing them again is the duplication the terse template exists to drop.
    assert_eq!(
        skill.body.matches("Watch a pull request.").count(),
        1,
        "{}",
        skill.body
    );
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

    assert!(
        skill
            .body
            .contains("- `repo` string = `\"current\"` — Repository to inspect."),
        "{}",
        skill.body
    );
    // No note and no default: the line is the signature and nothing else.
    assert!(skill.body.contains("- `deep` boolean\n"), "{}", skill.body);
    // The default is shown as a runnable value; the undeclared one as a
    // type-shaped placeholder.
    assert!(skill.body.contains("\"repo\":\"current\""));
    assert!(skill.body.contains("\"deep\":false"));
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

    assert!(
        skill
            .body
            .contains("- `pr`* number — The pull request number."),
        "{}",
        skill.body
    );
    assert!(skill.body.contains("- `payload`* json\n"), "{}", skill.body);
    assert!(skill.body.contains("\"pr\":0"));
    // Examples are type-shaped, so the body must explicitly say that a required
    // value absent from the operator's request cannot be invented from one.
    assert!(
        skill.body.contains(
            "ask the operator for every required `*` input they did not supply; never use an example placeholder as its value."
        ),
        "{}",
        skill.body
    );

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
fn fallback_passes_the_declared_input_object_to_the_cli() {
    let inputs = vec![
        WorkflowInput::new("pr", InputType::Number).required(),
        WorkflowInput::new("payload", InputType::Json).required(),
    ];
    let skill = render(&summary("babysit", "Watch a PR.", inputs));

    assert!(
        skill
            .body
            .contains("medulla workflow run babysit --inputs '{\"pr\":0,\"payload\":{}}'"),
        "{}",
        skill.body
    );
    assert!(!skill.body.contains("--inputs {\"id\":"), "{}", skill.body);
}

#[test]
fn fallback_fences_commands_with_backticks_in_the_workflow_id() {
    let skill = render(&summary("run`this", "Run a command.", vec![]));

    assert!(
        skill
            .body
            .contains("```sh\nmedulla workflow run 'run`this' --inputs '{}'\n```"),
        "{}",
        skill.body
    );
}

#[test]
fn a_condensed_input_note_points_at_the_tool_that_serves_it_whole() {
    let inputs = vec![WorkflowInput::new("ci_wait_secs", InputType::Number)
        .with_default(json!(1200))
        .with_description(
            "How long a pass will wait for in-flight CI checks to settle. This is a ceiling on a \
             watch, not a fixed sleep: it ends as soon as every check reaches a terminal state.",
        )];
    let skill = render(&summary("babysit", "Watch a PR.", inputs));

    assert!(
        skill.body.contains(
            "- `ci_wait_secs` number = `1200` — How long a pass will wait for in-flight \
                      CI checks to settle.\n"
        ),
        "{}",
        skill.body
    );
    assert!(
        skill
            .body
            .contains("`mcp__medulla__workflow_get` has the full description and input notes."),
        "{}",
        skill.body
    );
}

#[test]
fn an_uncondensed_signature_does_not_advertise_the_lookup() {
    let inputs = vec![WorkflowInput::new("pr", InputType::Number)
        .required()
        .with_description("The pull request number.")];
    let skill = render(&summary("babysit", "Watch a PR.", inputs));

    // Nothing was cut, so the extra sentence would be a token spent saying
    // "nothing was cut".
    assert!(!skill.body.contains("workflow_get"), "{}", skill.body);
}

#[test]
fn unterminated_short_text_does_not_advertise_the_lookup() {
    let inputs = vec![WorkflowInput::new("pr", InputType::Number)
        .required()
        .with_description("The pull request number")];
    let skill = render(&summary("babysit", "Watch a pull request", inputs));

    // `condense` supplies terminal punctuation for rendering, but neither
    // source was shortened, so the complete text is already in the skill.
    assert!(skill.description.starts_with("Watch a pull request."));
    assert!(skill.body.contains("The pull request number."));
    assert!(!skill.body.contains("workflow_get"), "{}", skill.body);
}

/// The budget this whole rewrite exists to meet.
///
/// Roughly four characters to a token, so 1200 characters is about 300 tokens
/// for a workflow with four thoroughly documented inputs — where the previous
/// template spent over a thousand. Asserted rather than merely intended,
/// because a template grows one well-meant clarifying sentence at a time, and
/// the cost is paid by every operator on every session.
#[test]
fn a_thoroughly_documented_workflow_stays_within_its_token_budget() {
    let skill = render(&verbose_summary());

    let frontmatter_len = skill.description.chars().count();
    assert!(
        frontmatter_len <= MAX_FRONTMATTER_DESCRIPTION_CHARS + 100,
        "frontmatter description is {frontmatter_len} chars: {}",
        skill.description
    );

    let body_len = skill.body.chars().count();
    assert!(
        body_len <= 1900,
        "body is {body_len} chars:\n{}",
        skill.body
    );

    // Everything the model needs is still in there.
    assert!(skill.body.contains("`pr`* number"));
    assert!(skill.body.contains("`ci_wait_secs` number = `1200`"));
    assert!(skill.body.contains("medulla skills install --with-mcp"));
}

#[test]
fn condense_prefers_sentence_boundaries_then_words() {
    // Fits whole, and is terminated so it can be joined to a following clause.
    assert_eq!(condense("Watch a PR", 40).as_deref(), Some("Watch a PR."));
    // Two sentences, only the first of which fits.
    assert_eq!(
        condense("Watch a PR. Then merge it once it is green.", 20).as_deref(),
        Some("Watch a PR.")
    );
    // Both fit, so both are kept.
    assert_eq!(
        condense("Watch a PR. Merge it.", 30).as_deref(),
        Some("Watch a PR. Merge it.")
    );
    // Not even the first sentence fits: cut on a word boundary and say so.
    assert_eq!(
        condense("Watch a pull request very closely indeed.", 20).as_deref(),
        Some("Watch a pull…")
    );
    // Newlines and runs of spaces are prose, not structure.
    assert_eq!(
        condense("Watch\n  a   PR", 40).as_deref(),
        Some("Watch a PR.")
    );
    // Nothing at all stays nothing, rather than becoming a lone period.
    assert_eq!(condense("   \n ", 40), None);
    // A single word past the cap is still cut rather than dropped.
    assert_eq!(
        condense("supercalifragilistic", 10).as_deref(),
        Some("supercali…")
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
