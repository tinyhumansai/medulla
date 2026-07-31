//! Focused tests for folding ACP stream updates into semantic harness events.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::daemon::providers::{Abort, RunTaskOptions};
use crate::daemon::status_detail;
use crate::tinyplace::HarnessProvider;

use super::FoldState;

#[test]
fn agent_message_chunks_form_one_reply() {
    let mut state = FoldState::new(None);
    for text in ["hello ", "world"] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": text}
        }))
        .unwrap();
        state.fold(update);
    }
    assert_eq!(state.reply(), "hello world");
}

#[test]
fn non_text_updates_do_not_pollute_the_reply() {
    let mut state = FoldState::new(None);
    let update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Run tests",
        "kind": "execute",
        "status": "pending"
    }))
    .unwrap();
    state.fold(update);
    assert_eq!(
        state.reply(),
        "ACP agent completed without a text response."
    );
}

#[test]
fn tool_updates_preserve_failure_state_for_the_copilot() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Terminal",
        "kind": "execute",
        "status": "in_progress",
        "rawInput": { "command": "cargo test --workspace" }
    }))
    .unwrap();
    let failure = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "failed",
        "rawOutput": "tests failed"
    }))
    .unwrap();
    let still_running = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress"
    }))
    .unwrap();

    state.fold(call);
    state.fold(still_running);
    state.fold(failure);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Terminal · $ cargo test --workspace\u{1f}call-1",
            "tool failed\u{1f}call-1"
        ]
    );
}

#[test]
fn running_tool_patch_surfaces_the_command_when_it_arrives_late() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Terminal",
        "kind": "execute",
        "status": "in_progress"
    }))
    .unwrap();
    let input = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress",
        "rawInput": { "command": "cargo test --workspace" }
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&input).unwrap()["rawInput"]["command"],
        "cargo test --workspace"
    );

    state.fold(call);
    state.fold(input);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Terminal\u{1f}call-1",
            "running Terminal · $ cargo test --workspace\u{1f}call-1"
        ]
    );
}

#[test]
fn running_tool_patch_preserves_initial_metadata() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Read configuration",
        "kind": "read",
        "status": "in_progress"
    }))
    .unwrap();
    let input = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress",
        "rawInput": { "path": "/tmp/medulla.json" }
    }))
    .unwrap();

    state.fold(call);
    state.fold(input);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Read: Read configuration\u{1f}call-1",
            "running Read · /tmp/medulla.json\u{1f}call-1"
        ]
    );
}

#[test]
fn thought_chunks_emit_a_cumulative_bounded_snapshot() {
    let thoughts = Arc::new(Mutex::new(Vec::new()));
    let captured = thoughts.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if event.event.kind == "agent_thought" {
            captured
                .lock()
                .unwrap()
                .push(event.event.payload["text"].as_str().unwrap().to_string());
        }
    })));
    for text in ["Checking ", "the workflow.", &"x".repeat(1_000)] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text }
        }))
        .unwrap();
        state.fold(update);
    }

    let thoughts = thoughts.lock().unwrap();
    assert_eq!(thoughts[0], "Checking ");
    assert_eq!(thoughts[1], "Checking the workflow.");
    assert_eq!(thoughts[2].chars().count(), 780);
    assert!(thoughts[2].starts_with('…'));
}

#[test]
fn thought_credentials_are_redacted_before_the_snapshot_is_bounded() {
    let thoughts = Arc::new(Mutex::new(Vec::new()));
    let captured = thoughts.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if event.event.kind == "agent_thought" {
            captured
                .lock()
                .unwrap()
                .push(event.event.payload["text"].as_str().unwrap().to_string());
        }
    })));
    let prefix = format!("{}sk-", "context ".repeat(110));
    for text in [prefix.as_str(), "abcdefghijklmnop0123456789"] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text }
        }))
        .unwrap();
        state.fold(update);
    }

    let final_thought = thoughts.lock().unwrap().last().unwrap().clone();
    assert!(final_thought.contains("[REDACTED]"));
    assert!(!final_thought.contains("0123456789"));
}

// ---------------------------------------------------------------------------
// Attribution reaches the ACP spawn path
// ---------------------------------------------------------------------------

/// A `RunTaskOptions` carrying `attribution`, with everything else inert.
fn attribution_options(attribution: bool) -> RunTaskOptions {
    RunTaskOptions {
        conversation: String::new(),
        session_class: crate::sessions::SessionClass::Bounded,
        resume_session_id: None,
        provider: HarnessProvider::Claude,
        prompt: String::new(),
        cwd: ".".to_string(),
        env: HashMap::new(),
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        attribution,
        on_event: None,
        on_stdin: None,
        on_session: None,
    }
}

/// `run_provider_task` dispatches to ACP *before* the spawn seam that applies
/// attribution for direct runs, so the ACP agent env must carry it itself —
/// otherwise every ACP-backed commit is unattributed.
#[cfg(unix)]
#[test]
fn agent_env_carries_attribution() {
    let env = super::acp_env(&attribution_options(true));
    assert!(
        env.contains_key("MEDULLA_ATTRIBUTION"),
        "ACP agent env must carry the attribution trailer"
    );
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0").map(String::as_str),
        Some("core.hooksPath"),
        "ACP agent env must activate the hook directory"
    );
}

/// Turning attribution off leaves the ACP env untouched.
#[test]
fn agent_env_omits_attribution_when_off() {
    let env = super::acp_env(&attribution_options(false));
    assert!(!env.contains_key("MEDULLA_ATTRIBUTION"));
    assert!(!env.contains_key("GIT_CONFIG_KEY_0"));
}
