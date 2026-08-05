//! What the shim puts on the wire — and, as much as anything here, what it
//! refuses to put there.

use super::*;

fn payload(json: serde_json::Value) -> serde_json::Value {
    json
}

#[test]
fn a_tool_event_names_the_tool() {
    let summary = summarize(
        HookEvent::PostToolUse,
        &payload(json!({ "tool_name": "Edit", "tool_input": { "file_path": "/etc/passwd" } })),
    );
    assert_eq!(summary, "used Edit");
    assert!(
        !summary.contains("passwd"),
        "the tool's input must not travel: {summary}"
    );
}

#[test]
fn a_prompt_travels_as_its_size_and_never_its_text() {
    let summary = summarize(
        HookEvent::UserPromptSubmit,
        &payload(json!({ "prompt": "deploy the thing with the password hunter2" })),
    );
    assert_eq!(summary, "prompt submitted (42 chars)");
    assert!(!summary.contains("hunter2"));
}

#[test]
fn a_notification_is_the_one_message_meant_for_the_operator() {
    assert_eq!(
        summarize(
            HookEvent::Notification,
            &payload(json!({ "message": "Claude needs your permission to use Bash" }))
        ),
        "Claude needs your permission to use Bash"
    );
}

#[test]
fn every_event_summarizes_without_any_payload_at_all() {
    for event in HookEvent::ALL {
        let summary = summarize(event, &serde_json::Value::Null);
        assert!(
            !summary.is_empty(),
            "{} produced no summary from an absent payload",
            event.as_str()
        );
    }
}

#[test]
fn a_session_start_names_its_source_when_the_harness_gives_one() {
    assert_eq!(
        summarize(
            HookEvent::SessionStart,
            &payload(json!({ "source": "resume" }))
        ),
        "session started (resume)"
    );
    assert_eq!(
        summarize(HookEvent::SessionStart, &payload(json!({}))),
        "session started"
    );
}
