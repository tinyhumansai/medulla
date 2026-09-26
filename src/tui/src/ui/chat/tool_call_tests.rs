//! Tests for the tool-call summariser.
//!
//! The payloads here are the ones observed in a live session — the transcript
//! that motivated this module rendered them as `⏺ tool({"managers": [{"id":…`.

use super::*;
use serde_json::json;

/// Nothing this module produces may contain raw JSON punctuation, whatever the
/// shape of the arguments. This is the invariant the old renderer broke.
fn assert_no_json(summary: &str) {
    for bad in ['{', '}', '[', ']'] {
        assert!(
            !summary.contains(bad),
            "summary leaked JSON punctuation {bad:?}: {summary}"
        );
    }
}

#[test]
fn a_bare_ledger_read_is_just_a_verb() {
    assert_eq!(summarize("host_list", &Value::Null), "listing hosts");
    assert_eq!(summarize("agent_list", &json!({})), "listing agents");
    assert_eq!(
        summarize("context_list", &Value::Null),
        "reading the environment index"
    );
}

#[test]
fn a_single_manager_deploy_names_it_and_its_objective() {
    let args = json!({"managers": [{
        "id": "news-fetch",
        "name": "News Fetcher",
        "objective": "Fetch and summarize today's top news headlines from a reputable source."
    }]});
    let summary = summarize("deploy_managers", &args);
    assert!(
        summary.starts_with("deploying manager News Fetcher — Fetch and summarize"),
        "{summary}"
    );
    assert_no_json(&summary);
}

#[test]
fn several_managers_lead_with_the_fan_out_width() {
    let args = json!({"managers": [
        {"id": "a", "name": "Alpha", "objective": "one"},
        {"id": "b", "name": "Beta", "objective": "two"},
        {"id": "c", "name": "Gamma", "objective": "three"},
    ]});
    let summary = summarize("deploy_managers", &args);
    assert_eq!(summary, "deploying 3 managers: Alpha, Beta, Gamma");
    assert_no_json(&summary);
}

#[test]
fn a_bare_manager_array_is_accepted_as_well_as_the_wrapper() {
    // Some providers emit the list itself rather than {"managers": [...]}.
    let args = json!([{"name": "Solo", "objective": "do the thing"}]);
    assert_eq!(
        summarize("deploy_managers", &args),
        "deploying manager Solo — do the thing"
    );
}

#[test]
fn delegation_names_the_agent_and_the_instruction() {
    let args = json!({
        "agentId": "this-device",
        "tasks": [{"instruction": "Fetch today's top news headlines from a reputable source."}]
    });
    let summary = summarize("delegate_tasks", &args);
    assert!(
        summary.starts_with("delegating a task to this-device — Fetch today's"),
        "{summary}"
    );
    assert_no_json(&summary);
}

#[test]
fn several_tasks_are_counted_not_listed() {
    let args =
        json!({"agentId": "worker-2", "tasks": [{"instruction": "a"}, {"instruction": "b"}]});
    assert_eq!(
        summarize("delegate_tasks", &args),
        "delegating 2 tasks to worker-2"
    );
}

#[test]
fn a_targeted_call_reads_as_verb_plus_target() {
    assert_eq!(
        summarize("agent_capabilities", &json!({"agentId": "this-device"})),
        "probing this-device"
    );
    assert_eq!(
        summarize("manager_stop", &json!({"managerId": "news-fetch"})),
        "stopping news-fetch"
    );
    assert_eq!(
        summarize("harness_select", &json!({"harness": "claude"})),
        "selecting harness claude"
    );
}

#[test]
fn a_missing_target_still_yields_the_verb_rather_than_a_placeholder() {
    assert_eq!(summarize("agent_capabilities", &json!({})), "probing");
    assert_eq!(summarize("manager_send", &Value::Null), "steering");
}

#[test]
fn a_harness_tool_keeps_the_familiar_name_and_argument_shape() {
    // Bash/Read/WebSearch already read well; only the orchestrator's own
    // vocabulary is rewritten into prose.
    assert_eq!(
        summarize("Bash", &json!({"command": "ls -la"})),
        "Bash(ls -la)"
    );
    assert_eq!(summarize("Read", &json!({"path": "a.rs"})), "Read(a.rs)");
    assert_eq!(summarize("Bash", &json!({})), "Bash");
}

#[test]
fn an_unknown_tool_never_inlines_a_nested_blob() {
    // The failure this module exists to prevent: a nested object rendered whole.
    let args = json!({"payload": {"deep": {"deeper": "value"}}, "note": "hello"});
    let summary = summarize("mystery_tool", &args);
    assert_no_json(&summary);
    assert_eq!(summary, "mystery_tool(hello)");
}

#[test]
fn a_nameless_call_is_described_rather_than_called_tool() {
    // Streamed calls whose `tool_call_start` never arrived used to render as
    // `⏺ tool`, which reads like a tool named "tool".
    assert_eq!(summarize("", &Value::Null), "calling a tool");
    assert_eq!(summarize("   ", &json!({"a": 1})), "calling a tool");
}

#[test]
fn long_free_text_is_clipped_on_a_word_boundary_marker() {
    let long = "x".repeat(200);
    let summary = summarize("context_search", &json!({"query": long}));
    assert!(summary.contains('…'), "{summary}");
    assert!(
        summary.chars().count() < 100,
        "clipped summary is still too long: {}",
        summary.chars().count()
    );
}

#[test]
fn whitespace_and_newlines_are_flattened_to_one_line() {
    let args = json!({"managers": [{"name": "M", "objective": "line one\n\nline two\tend"}]});
    let summary = summarize("deploy_managers", &args);
    assert!(!summary.contains('\n'), "{summary}");
    assert_eq!(summary, "deploying manager M — line one line two end");
}

#[test]
fn an_untyped_inference_end_yields_its_named_calls() {
    // The exact shape the backend SSE stream sends: `inference_end` is not
    // modelled by `EventKind`, so the renderer reads `toolCalls` from the raw
    // payload. Without this the names never arrive and every call rendered as
    // an anonymous fragment.
    let payload = json!({
        "id": "inf:1",
        "tier": "orchestrator",
        "toolCalls": [
            {"name": "host_list", "args": {}},
            {"name": "agent_list", "args": {}},
        ],
    });
    let calls = calls_from_payload(payload.as_object().expect("object"));
    let rendered: Vec<String> = calls
        .iter()
        .map(|(name, args)| summarize(name, args))
        .collect();
    assert_eq!(rendered, vec!["listing hosts", "listing agents"]);
}

#[test]
fn json_encoded_arguments_are_parsed_rather_than_shown_as_text() {
    // Some providers send `args` as a JSON string, not an object.
    let payload = json!({"toolCalls": [
        {"name": "deploy_managers", "args": "{\"managers\":[{\"name\":\"News Fetcher\"}]}"}
    ]});
    let calls = calls_from_payload(payload.as_object().expect("object"));
    assert_eq!(calls.len(), 1);
    assert_eq!(
        summarize(&calls[0].0, &calls[0].1),
        "deploying manager News Fetcher"
    );
}

#[test]
fn an_unparseable_argument_string_still_renders_the_call() {
    let payload = json!({"toolCalls": [{"name": "Bash", "args": "not json"}]});
    let calls = calls_from_payload(payload.as_object().expect("object"));
    assert_eq!(summarize(&calls[0].0, &calls[0].1), "Bash");
}

#[test]
fn a_payload_with_no_tool_calls_yields_nothing() {
    let payload = json!({"id": "inf:2", "tier": "orchestrator"});
    assert!(calls_from_payload(payload.as_object().expect("object")).is_empty());
}

#[test]
fn harness_select_reads_the_harness_id_the_orchestrator_actually_sends() {
    // Observed live: the argument is `harnessId`, not `harness`.
    assert_eq!(
        summarize(
            "harness_select",
            &json!({"harnessId": "harness:-Os3ZyUdvyTEO4JDAAAP"})
        ),
        "selecting harness harness:-Os3ZyUdvyTEO4JDAAAP"
    );
}
