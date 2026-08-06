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
fn control_bytes_in_a_notification_message_are_stripped() {
    let summary = summarize(
        HookEvent::Notification,
        &payload(json!({ "message": "clear\x1b[2Jscreen\x07bell" })),
    );
    assert_eq!(summary, "clear[2Jscreenbell");
    assert!(
        !summary.chars().any(|c| c.is_control()),
        "a control byte reached the summary: {summary:?}"
    );
}

#[test]
fn control_bytes_in_a_tool_name_are_stripped() {
    let summary = summarize(
        HookEvent::PostToolUse,
        &payload(json!({ "tool_name": "Ed\x1b]0;pwned\x07it" })),
    );
    assert!(
        !summary.chars().any(|c| c.is_control()),
        "a control byte reached the summary: {summary:?}"
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

/// The P2 Codex found on this branch: the declared timeout has to match
/// `harness_hooks::builtin`'s own per-event value, or the shim's single
/// deadline (see [`run_hook_cmd`]'s docs) would be built from the wrong
/// number and the whole point of deriving both budgets from one deadline is
/// lost.
#[test]
fn declared_timeout_matches_the_builtins_per_event_value() {
    assert_eq!(
        declared_timeout(HookEvent::SessionEnd),
        Duration::from_secs(3),
        "must match builtin::SESSION_END_TIMEOUT_SECS"
    );
    for event in HookEvent::ALL {
        if event != HookEvent::SessionEnd {
            assert_eq!(declared_timeout(event), Duration::from_secs(5));
        }
    }
}

/// The whole reason for one deadline: the stdin read and the report used to
/// draw from separate budgets (500ms + 3s), so a slow read alone could push
/// the shim's total past `SessionEnd`'s 3-second declared timeout before the
/// report even started. Deriving both from one deadline structurally rules
/// that out — the read's own budget shrinks to fit whatever is left, so the
/// two together can never exceed what was declared, minus headroom.
#[test]
fn the_read_and_report_budgets_can_never_together_exceed_the_declared_timeout() {
    for event in HookEvent::ALL {
        let declared = declared_timeout(event);
        // The read spends at most `READ_BUDGET`; the report gets whatever the
        // one deadline leaves. Both are reserved out of `declared - HEADROOM`,
        // so the read's own constant must fit inside that window with room to
        // report — asserted against the constants themselves, with no clock
        // read to cancel the arithmetic out.
        assert!(
            HEADROOM < declared,
            "{}: the headroom must fit inside the declared timeout",
            event.as_str()
        );
        assert!(
            READ_BUDGET + HEADROOM < declared,
            "{}: read ({READ_BUDGET:?}) + headroom ({HEADROOM:?}) leaves nothing \
             of the declared {declared:?} for the report",
            event.as_str()
        );
    }
}

/// Once the shim's deadline has already passed — an unlikely but possible
/// case if, say, process startup itself ran long — the read must give up on
/// the very next poll rather than still trying for `READ_BUDGET`.
#[test]
fn read_payload_gives_up_immediately_once_the_deadline_has_already_passed() {
    let deadline = Instant::now() - Duration::from_millis(1);
    assert_eq!(read_payload(deadline), serde_json::Value::Null);
}
