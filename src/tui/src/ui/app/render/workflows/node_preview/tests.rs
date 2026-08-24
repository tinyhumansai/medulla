//! Focused tests for kind-aware workflow step previews.

use ratatui::text::Line;
use serde_json::json;

use medulla::workflows::{RunRecord, RunStatus, RunStep};

use super::{kind_lines, run_header, run_lines, AgentDefaults};

/// Flatten styled lines into the text an operator reads.
fn text(lines: Vec<Line<'static>>) -> String {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn code_steps_have_a_language_badge_and_numbered_source() {
    let preview = text(kind_lines(
        "code",
        &json!({
            "language": "python",
            "source": "def greet(name):\n    return f\"hi {name}\""
        }),
        80,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("python"), "{preview}");
    assert!(preview.contains("1 │ def greet(name):"), "{preview}");
    assert!(preview.contains("2 │     return"), "{preview}");
}

#[test]
fn shell_tool_steps_use_the_same_code_viewer() {
    let preview = text(kind_lines(
        "tool_call",
        &json!({
            "slug": "medulla:shell",
            "args": { "language": "shell", "script": "cargo test\ncargo clippy" }
        }),
        80,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("executable source"), "{preview}");
    assert!(preview.contains("1 │ cargo test"), "{preview}");
    assert!(preview.contains("2 │ cargo clippy"), "{preview}");
}

#[test]
fn shell_tool_steps_redact_credentials_before_highlighting() {
    let preview = text(kind_lines(
        "tool_call",
        &json!({
            "slug": "medulla:shell",
            "args": {
                "language": "shell",
                "script": "curl -H 'Authorization: Basic dXNlcjpwYXNz' https://example.test\nPASSWORD=short\nAPI_KEY=abcdefgh curl -H 'Authorization: Basic bWl4ZWQtc2VjcmV0' https://example.test\ncurl HTTPS://upper:private@example.test/upper?x=private\npsql postgres://dbuser:dbpass@db.example.test/app\necho visible"
            }
        }),
        100,
        &AgentDefaults::default(),
    ));

    for secret in [
        "dXNlcjpwYXNz",
        "short",
        "abcdefgh",
        "bWl4ZWQtc2VjcmV0",
        "upper:private",
        "x=private",
        "dbuser:dbpass",
    ] {
        assert!(!preview.contains(secret), "{preview}");
    }
    assert!(
        preview.contains("credential-bearing source redacted"),
        "{preview}"
    );
    assert!(preview.contains("HTTPS://example.test/upper"), "{preview}");
    assert!(
        preview.contains("postgres://db.example.test/app"),
        "{preview}"
    );
    assert!(preview.contains("echo visible"), "{preview}");
}

#[test]
fn wrapped_code_keeps_a_blank_gutter_on_continuation_lines() {
    let lines = kind_lines(
        "code",
        &json!({
            "language": "shell",
            "source": "printf '%s' \"$pr\" | jq -c --argjson result \"$roll\""
        }),
        24,
        &AgentDefaults::default(),
    );
    let rendered = lines
        .iter()
        .skip(1)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.len() > 1, "{rendered:?}");
    assert!(rendered[0].starts_with("1 │ "), "{rendered:?}");
    assert!(
        rendered.iter().skip(1).all(|line| line.starts_with("  │ ")),
        "{rendered:?}"
    );
    assert!(lines.iter().skip(1).all(|line| line.width() <= 24));
}

#[test]
fn generic_detail_redacts_credential_shaped_fields_recursively() {
    let preview = text(kind_lines(
        "http_request",
        &json!({
            "method": "POST",
            "url": "https://example.test",
            "headers": {
                "Authorization": "Bearer private",
                "X-API-Key": "hyphen-private",
                "Cookie": "session=cookie-private",
                "X-Trace": "visible"
            },
            "api_key": "private",
            "nested": {
                "apiKey": "camel-private",
                "callback": "https://user:pass@example.test/hook?token=url-private"
            }
        }),
        80,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("POST"), "{preview}");
    assert!(preview.contains("X-Trace"), "{preview}");
    assert!(preview.contains("visible"), "{preview}");
    assert!(!preview.contains("Bearer private"), "{preview}");
    assert!(!preview.contains("\"private\""), "{preview}");
    assert!(!preview.contains("camel-private"), "{preview}");
    assert!(!preview.contains("hyphen-private"), "{preview}");
    assert!(!preview.contains("cookie-private"), "{preview}");
    assert!(!preview.contains("user:pass"), "{preview}");
    assert!(!preview.contains("url-private"), "{preview}");
    assert!(preview.contains("••••"), "{preview}");
}

#[test]
fn http_preview_strips_url_credentials_query_and_fragment() {
    let preview = text(kind_lines(
        "http_request",
        &json!({
            "method": "POST",
            "url": "https://user:pass@example.test/hook?access_token=secret#private"
        }),
        80,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("https://example.test/hook"), "{preview}");
    for secret in ["user", "pass", "access_token", "secret", "private"] {
        assert!(!preview.contains(secret), "{preview}");
    }
}

/// Host config pinning `harness` and `model`, with no workflow-level block —
/// the layers beneath a node before this feature existed.
fn host_defaults(harness: &str, model: &str) -> AgentDefaults {
    let config = medulla::config::WorkflowsConfig {
        default_worker: "fallback".into(),
        default_provider: medulla::protocol::HarnessProvider::from_wire(harness),
        default_model: model.into(),
        ..Default::default()
    };
    AgentDefaults::new(&config, &Default::default())
}

/// Host config as above, with a workflow that pins its own harness over it.
fn workflow_defaults(harness: &str, model: &str) -> AgentDefaults {
    let config = medulla::config::WorkflowsConfig {
        default_worker: "fallback".into(),
        default_provider: Some(medulla::protocol::HarnessProvider::Claude),
        default_model: "claude-opus-4".into(),
        ..Default::default()
    };
    AgentDefaults::new(
        &config,
        &medulla::workflows::WorkflowDefaults {
            harness: Some(harness.into()),
            model: Some(model.into()),
        },
    )
}

#[test]
fn agent_steps_explain_the_effective_worker_harness_and_dynamic_task() {
    let preview = text(kind_lines(
        "agent",
        &json!({
            "agent_ref": "reviewer",
            "prompt": "=item.pull_request"
        }),
        100,
        &host_defaults("codex", "gpt-5.6"),
    ));

    assert!(preview.contains("agent    reviewer"), "{preview}");
    assert!(preview.contains("named workflow agent"), "{preview}");
    assert!(preview.contains("harness  Codex"), "{preview}");
    assert!(preview.contains("model gpt-5.6"), "{preview}");
    assert!(preview.contains("host default"), "{preview}");
    assert!(
        preview.contains("Uses “pull_request” from the previous step"),
        "{preview}"
    );
    assert!(preview.contains("binding  =item.pull_request"), "{preview}");
}

#[test]
fn concatenated_agent_prompt_is_unescaped_and_names_its_dynamic_input() {
    let preview = text(kind_lines(
        "agent",
        &json!({
            "agent_ref": "reviewer",
            "prompt": "=(\"Review each PR in order.\\n\\nFinish with one line per PR.\" + .nodes.collect.item.json.output.report)"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("prompt template"), "{preview}");
    assert!(preview.contains("Review each PR in order.\n"), "{preview}");
    assert!(
        preview.contains("Finish with one line per PR."),
        "{preview}"
    );
    assert!(preview.contains("${collect.report}"), "{preview}");
    assert!(
        preview.contains("dynamic input  collect → report"),
        "{preview}"
    );
    assert!(!preview.contains("\\n"), "{preview}");
    assert!(!preview.contains("=(\""), "{preview}");
}

#[test]
fn a_piped_prompt_operand_is_named_by_the_value_it_renders() {
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Apply fixes for these findings: \" + (.item.json.json.findings | tostring)"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(
        preview.contains("Apply fixes for these findings:"),
        "{preview}"
    );
    assert!(preview.contains("${findings}"), "{preview}");
    assert!(
        preview.contains("dynamic input  previous step → findings"),
        "{preview}"
    );
    // The jq machinery is what this decoding exists to remove; leaving it in
    // the prose is the regression.
    assert!(!preview.contains("tostring"), "{preview}");
}

#[test]
fn a_conditional_prompt_operand_is_named_by_the_value_it_tests() {
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Review the repository \" + .inputs.repo + \". \" + (if .inputs.include_tests then \"Read the test suite too.\" else \"Skip the test suite.\" end)"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(
        preview.contains("Review the repository ${inputs.repo}"),
        "{preview}"
    );
    assert!(preview.contains("${if include_tests}"), "{preview}");
    assert!(
        preview.contains("inputs.include_tests → one of two texts"),
        "{preview}"
    );
    assert!(preview.contains("workflow input → repo"), "{preview}");
    assert!(!preview.contains("then \"Read"), "{preview}");
}

#[test]
fn an_alternative_prompt_operand_is_named_by_its_preferred_value() {
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Pull request \" + (.nodes.assess.item.pr.url // (\"#\" + (.nodes.assess.item.pr.number | tostring)))"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("${assess.url}"), "{preview}");
    assert!(preview.contains("assess → url"), "{preview}");
    assert!(!preview.contains("//"), "{preview}");
}

#[test]
fn an_undecodable_prompt_operand_is_reported_rather_than_hidden() {
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "=\"Files: \" + ([.nodes.survey.item.files[].path] | join(\", \"))" }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("Files: ${value}"), "{preview}");
    // Nothing this module could not explain may vanish: the line that names it
    // is the operator's only route back to what fills the placeholder.
    assert!(preview.contains("join("), "{preview}");
}

#[test]
fn agent_run_detail_shows_the_resolved_prompt_and_plain_reply() {
    let run = RunRecord {
        id: "run-1".into(),
        workflow_id: "demo".into(),
        status: RunStatus::Succeeded,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![RunStep {
            node_id: "agent".into(),
            status: "success".into(),
            duration_ms: 3,
            input: Some(json!("Review every changed file\nand explain the risk.")),
            output: Some(json!([
                {
                    "json": {
                        "text": "The change is safe.\nTests cover the edge case.",
                        "worker": "builder"
                    }
                }
            ])),
            diagnostics: Vec::new(),
            transcript: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
        inputs: Default::default(),
        trigger: None,
        origin: None,
        executor: None,
        cancel_requested: false,
        summary: None,
        diagnosis: None,
    };

    let preview = text(run_lines(&run, "agent", true));

    assert!(preview.contains("prompt"), "{preview}");
    assert!(preview.contains("ran as"), "{preview}");
    assert!(preview.contains("builder"), "{preview}");
    assert!(preview.contains("Review every changed file"), "{preview}");
    assert!(preview.contains("and explain the risk."), "{preview}");
    assert!(preview.contains("output"), "{preview}");
    assert!(preview.contains("The change is safe."), "{preview}");
    assert!(preview.contains("Tests cover the edge case."), "{preview}");
    assert!(!preview.contains("\"json\""), "{preview}");
}

#[test]
fn older_agent_run_labels_missing_evidence_as_output() {
    let run = RunRecord {
        id: "run-old".into(),
        workflow_id: "demo".into(),
        status: RunStatus::Succeeded,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![RunStep {
            node_id: "agent".into(),
            status: "success".into(),
            duration_ms: 3,
            input: None,
            output: None,
            diagnostics: Vec::new(),
            transcript: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
        inputs: Default::default(),
        trigger: None,
        origin: None,
        executor: None,
        cancel_requested: false,
        summary: None,
        diagnosis: None,
    };

    let preview = text(run_lines(&run, "agent", true));
    assert!(preview.contains("output  unavailable"), "{preview}");
    assert!(!preview.contains("result  unavailable"), "{preview}");
}

#[test]
fn a_step_that_names_its_own_harness_shows_that_one_and_says_so() {
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "fix it", "harness": "codex", "model": "gpt-5-codex" }),
        100,
        &workflow_defaults("claude", "claude-opus-4"),
    ));

    assert!(preview.contains("harness  Codex"), "{preview}");
    assert!(preview.contains("model gpt-5-codex"), "{preview}");
    assert!(preview.contains("this step"), "{preview}");
}

#[test]
fn a_step_that_names_nothing_shows_the_workflow_s_own_choice() {
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "fix it" }),
        100,
        &workflow_defaults("codex", "gpt-5-codex"),
    ));

    assert!(preview.contains("harness  Codex"), "{preview}");
    assert!(preview.contains("workflow default"), "{preview}");
}

#[test]
fn a_step_switching_harness_does_not_show_the_inherited_model() {
    // The pane must not claim a Claude model id will be sent to Codex — that is
    // precisely what the dispatch refuses to do.
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "fix it", "harness": "codex" }),
        100,
        &workflow_defaults("claude", "claude-opus-4"),
    ));

    assert!(preview.contains("harness  Codex"), "{preview}");
    assert!(!preview.contains("claude-opus-4"), "{preview}");
    assert!(preview.contains("model worker default"), "{preview}");
}

#[test]
fn a_custom_preset_is_shown_by_its_id() {
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "fix it", "harness": "deepseek-claude" }),
        100,
        &host_defaults("claude", "claude-opus-4"),
    ));

    assert!(preview.contains("harness  deepseek-claude"), "{preview}");
}

#[test]
fn a_hand_edited_harness_that_cannot_be_read_says_so_rather_than_showing_a_fallback() {
    let preview = text(kind_lines(
        "agent",
        &json!({ "prompt": "fix it", "harness": "claude code" }),
        100,
        &host_defaults("claude", "claude-opus-4"),
    ));

    assert!(preview.contains("custom harness id"), "{preview}");
}

#[test]
fn a_wrapped_operand_behind_a_fallback_is_still_named_by_its_value() {
    // `(… | tostring) // "none"`: the fallback splits first, and its head is
    // itself parenthesized. Classifying that head directly finds no path, and
    // the preview used to give up and print the jq source.
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Findings: \" + ((.item.json.json.findings | tostring) // \"none\")"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("Findings: ${findings}"), "{preview}");
    assert!(
        preview.contains("dynamic input  previous step → findings"),
        "{preview}"
    );
    assert!(!preview.contains("tostring"), "{preview}");
    assert!(!preview.contains("${value}"), "{preview}");
}

#[test]
fn a_wrapped_operand_with_nothing_legible_keeps_its_whole_source() {
    // The other half of the same rule: when peeling the head still finds no
    // path, the operator sees the complete expression rather than the fragment
    // the split happened to cut.
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Files: \" + (([.nodes.survey.item.files[].path] | join(\", \")) // \"none\")"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("Files: ${value}"), "{preview}");
    assert!(preview.contains("join("), "{preview}");
    // The fallback is part of what fills the placeholder, so it is not dropped.
    assert!(preview.contains("none"), "{preview}");
}

#[test]
fn a_condition_whose_field_contains_then_is_not_cut_mid_identifier() {
    // `.inputs.authenticated` has `then` inside it. A substring search for the
    // keyword split the condition at `au|thenticated`, naming the operand
    // `${if au}` after a fragment of the field it was meant to report.
    let preview = text(kind_lines(
        "agent",
        &json!({
            "prompt": "=\"Use \" + (if .inputs.authenticated then \"secure\" else \"open\" end)"
        }),
        100,
        &AgentDefaults::default(),
    ));

    assert!(preview.contains("${if authenticated}"), "{preview}");
    assert!(
        preview.contains("inputs.authenticated → one of two texts"),
        "{preview}"
    );
    assert!(!preview.contains("${if au}"), "{preview}");
}

#[test]
fn the_run_header_says_what_the_run_was_given_and_who_asked_for_it() {
    // The step's own evidence cannot say any of this, and without it the pane
    // answered "what did this step do" while leaving "in aid of what" open.
    let mut record = medulla::workflows::new_run_record("run-abc-1234abcd", "sweep", 1_000)
        .with_inputs(
            &json!({ "repo": "acme/api" }).as_object().cloned().unwrap(),
            &json!({}),
        )
        .with_origin(Some(medulla::workflows::RunOrigin::session(
            "pty-0000-feedface",
        )));
    record.status = RunStatus::Succeeded;
    record.finished_at = Some(1_000 + 95_000);
    record.summary = Some("Reviewed acme/api.".into());

    let header = text(run_header(&record));
    assert!(
        header.contains("1234abcd · succeeded · 1m 35s · 0 steps"),
        "{header}"
    );
    assert!(header.contains("repo=acme/api"), "{header}");
    assert!(header.contains("feedface"), "{header}");
    assert!(header.contains("Reviewed acme/api."), "{header}");
}

#[test]
fn the_run_header_of_a_workflow_with_no_arguments_omits_the_inputs_line() {
    let record = medulla::workflows::new_run_record("run-1", "sweep", 1_000);
    let header = text(run_header(&record));
    assert!(header.contains("running"), "{header}");
    assert!(!header.contains("\nin   "), "nothing to show: {header}");
    assert!(!header.contains("\nfrom "), "nobody claimed it: {header}");
}

#[test]
fn a_multi_line_input_is_flattened_into_the_header_digest() {
    // The header is one line per fact; an input carrying a pasted paragraph
    // must not turn it into ten.
    let record = medulla::workflows::new_run_record("run-1", "sweep", 1_000).with_inputs(
        &json!({ "note": "first line\nsecond line" })
            .as_object()
            .cloned()
            .unwrap(),
        &json!({}),
    );
    let header = text(run_header(&record));
    assert!(header.contains("note=first line second line"), "{header}");
}
