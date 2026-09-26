//! Tests for progress-frame classification.
//!
//! The round-trip tests are the point of this file. `classify` parses a string
//! another module formats, which is the kind of coupling that rots quietly: the
//! producer gets reworded, the reader keeps compiling, and tool calls stop being
//! drawn with no test going red. So the cases below run real harness events
//! through [`crate::daemon::status::status_detail`] and classify *its* output
//! rather than a hand-written copy of it.

use serde_json::json;

use crate::daemon::status_detail;
use crate::protocol::HarnessEvent;

use super::{classify, Progress};

/// Classify what `status_detail` actually produces for `event`.
fn round_trip(event: &HarnessEvent) -> Progress {
    let frame = status_detail(event).expect("this event has a status line");
    classify(&frame)
}

#[test]
fn a_tool_call_frame_is_recognised_as_the_call_it_describes() {
    let event = HarnessEvent {
        kind: "tool_call".to_string(),
        role: "agent".to_string(),
        payload: json!({
            "call_id": "c1",
            "tool_name": "workflow_apply_ops",
            "display": "add node notify",
        }),
        ..Default::default()
    };
    assert_eq!(
        round_trip(&event),
        Progress::Tool {
            call_id: Some("c1".to_string()),
            text: "Workflow Apply Ops: add node notify".to_string(),
        }
    );
}

#[test]
fn the_chatter_around_a_tool_call_stays_status() {
    // These are the frames that age out of the transcript. Misreading any of
    // them as a tool call would put permanent `⏺` lines in the scrollback.
    let message = HarnessEvent {
        kind: "agent_message".to_string(),
        payload: json!({ "text": "done" }),
        ..Default::default()
    };
    assert!(matches!(round_trip(&message), Progress::Status(_)));

    let thinking = HarnessEvent {
        kind: "agent_thinking".to_string(),
        payload: json!({ "text": "hmm" }),
        ..Default::default()
    };
    assert_eq!(round_trip(&thinking), Progress::Thinking("hmm".to_string()));
}

#[test]
fn tool_settlement_is_promoted_so_the_call_can_update_in_place() {
    let event = HarnessEvent {
        kind: "tool_result".to_string(),
        payload: json!({
            "call_id": "c",
            "ok": false,
            "is_error": true,
            "exit_code": 2,
            "output": "",
            "output_bytes": 0
        }),
        ..Default::default()
    };

    assert_eq!(
        round_trip(&event),
        Progress::ToolResult {
            failed: true,
            detail: "exit 2".to_string(),
            call_id: Some("c".to_string()),
        }
    );
}

#[test]
fn a_providers_own_wording_passes_through_untouched() {
    let frame = "compiling the workspace";
    assert_eq!(classify(frame), Progress::Status(frame.to_string()));
}

#[test]
fn a_prefix_with_no_call_after_it_is_not_promoted_to_a_tool_line() {
    // "running " alone says only that something happened. A `⏺` line with
    // nothing on it is worse than the status line it came from.
    assert_eq!(
        classify("running "),
        Progress::Status("running ".to_string())
    );
    assert_eq!(
        classify("running    "),
        Progress::Status("running    ".to_string())
    );
}

#[test]
fn a_frame_that_merely_contains_the_word_running_is_not_a_tool_call() {
    // Anchored at the start, not searched for: a status line reporting that a
    // workflow is running is not the copilot calling a tool.
    let frame = "the workflow is running";
    assert_eq!(classify(frame), Progress::Status(frame.to_string()));
}

#[test]
fn an_empty_frame_is_reported_rather_than_swallowed() {
    // Dropping it here would hide a provider emitting blanks; the transcript's
    // own dedup is what collapses them.
    assert_eq!(classify(""), Progress::Status(String::new()));
}
