//! Unit tests for transcript folding and its bounds.

use crate::daemon::mappers::HarnessSemanticEvent;
use crate::protocol::HarnessEvent;

use super::*;

/// One semantic event of `kind` carrying `payload`.
fn event(kind: &str, payload: serde_json::Value) -> HarnessSemanticEvent {
    HarnessSemanticEvent {
        line: 0,
        timestamp_ms: 1_700_000_000_000,
        record_type: format!("test:{kind}"),
        event: HarnessEvent {
            kind: kind.to_string(),
            payload,
            ..Default::default()
        },
    }
}

/// The three kinds that carry prose are recorded verbatim — this is the whole
/// account of a turn, so nothing about the wording may be invented here.
#[test]
fn prose_events_are_recorded_verbatim() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event(
        "agent_message",
        serde_json::json!({"text": "on it"}),
    ));
    collector.observe(&event("agent_thinking", serde_json::json!({"text": "hmm"})));

    let entries = collector.finish();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].kind, "agent_message");
    assert_eq!(entries[0].text, "on it");
    assert_eq!(entries[0].at_ms, 1_700_000_000_000);
    assert_eq!(entries[1].kind, "agent_thinking");
    assert_eq!(entries[1].text, "hmm");
}

/// A tool row reads as the harness's own summary, qualified by the tool that
/// produced it — `Bash(npm test)` rather than either half alone.
#[test]
fn a_tool_call_reads_as_name_and_display() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event(
        "tool_call",
        serde_json::json!({"tool_name": "Bash", "display": "npm test"}),
    ));

    let entries = collector.finish();
    assert_eq!(entries[0].text, "Bash(npm test)");
}

/// A harness that supplied no summary still gets a row: the bare name says
/// more than a dropped event.
#[test]
fn a_tool_call_without_a_summary_falls_back_to_its_name() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event(
        "tool_call",
        serde_json::json!({"tool_name": "Read"}),
    ));

    assert_eq!(collector.finish()[0].text, "Read");
}

/// Successful results are skipped and failures are kept. The budget exists to
/// hold the parts of a turn that explain a surprising outcome, and a wall of
/// successful output is what would push those past the cap.
#[test]
fn only_failing_tool_results_are_recorded() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event(
        "tool_result",
        serde_json::json!({"ok": true, "output": "all good"}),
    ));
    collector.observe(&event(
        "tool_result",
        serde_json::json!({"ok": false, "exit_code": 2, "output": "boom"}),
    ));

    let entries = collector.finish();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "failed (exit 2): boom");
}

/// A result the harness itself flagged as an error is kept even when it also
/// claims `ok` — the two disagree, and the flag is the one worth believing.
#[test]
fn a_result_flagged_as_an_error_is_recorded_despite_ok() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event(
        "tool_result",
        serde_json::json!({"ok": true, "is_error": true, "output": "denied"}),
    ));

    assert_eq!(collector.finish()[0].text, "failed: denied");
}

/// Status, lifecycle and approval events carry no line to read back; the work
/// snapshot and the live status frames already render them.
#[test]
fn non_narrative_kinds_are_skipped() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event("status", serde_json::json!({"state": "running"})));
    collector.observe(&event(
        "lifecycle",
        serde_json::json!({"phase": "turn_start"}),
    ));
    collector.observe(&event(
        "approval_request",
        serde_json::json!({"tool_name": "Bash"}),
    ));

    assert!(collector.finish().is_empty());
}

/// An unknown kind is skipped rather than recorded as a blank row: it decodes
/// to `Unknown`, and there is nothing to show.
#[test]
fn an_unknown_kind_is_skipped() {
    let mut collector = TranscriptCollector::new();
    collector.observe(&event("something_new", serde_json::json!({"a": 1})));

    assert!(collector.finish().is_empty());
}

/// Overflow is announced. A transcript that simply stops is indistinguishable
/// from a harness that simply stopped, which is the wrong conclusion.
#[test]
fn overflow_appends_a_note_saying_how_much_was_lost() {
    let mut collector = TranscriptCollector::new();
    for index in 0..MAX_ENTRIES + 3 {
        collector.push("agent_message", &format!("line {index}"));
    }

    let entries = collector.finish();
    assert_eq!(entries.len(), MAX_ENTRIES + 1);
    let note = entries.last().expect("a note is owed");
    assert_eq!(note.kind, "truncated");
    assert!(
        note.text.contains("3 further events"),
        "the note must count what was dropped: {}",
        note.text
    );
}

/// The byte budget bites independently of the entry count, so a handful of
/// enormous entries cannot make a run record unbounded.
#[test]
fn the_byte_budget_stops_recording_before_the_entry_count_does() {
    let mut collector = TranscriptCollector::new();
    let big = "x".repeat(2 * 1024);
    for _ in 0..100 {
        collector.push("agent_message", &big);
    }

    let entries = collector.finish();
    let bytes: usize = entries.iter().map(|entry| entry.text.len()).sum();
    assert!(
        entries.len() < 100,
        "the budget must bite: {}",
        entries.len()
    );
    assert!(
        bytes <= MAX_TEXT_BYTES + 2 * 1024 + 128,
        "retained {bytes} bytes, over budget"
    );
}

/// A single oversized entry keeps its head *and* its tail: the last lines of a
/// failing command are usually the error, and truncating from the end throws
/// away the half that says how it turned out.
#[test]
fn an_oversized_entry_is_elided_in_the_middle() {
    let mut collector = TranscriptCollector::new();
    let text = format!("HEAD{}TAIL", "m".repeat(8 * 1024));
    collector.push("tool_result", &text);

    let entry = &collector.finish()[0];
    assert!(entry.text.starts_with("HEAD"), "{}", entry.text);
    assert!(entry.text.ends_with("TAIL"), "{}", entry.text);
    assert!(entry.text.contains('…'), "elision is marked");
    assert!(entry.text.len() < text.len());
}

/// Eliding a multi-byte string must not split a character — the result is a
/// `String`, and a split boundary would panic rather than truncate.
#[test]
fn elision_respects_character_boundaries() {
    let mut collector = TranscriptCollector::new();
    collector.push("agent_message", &"é".repeat(4 * 1024));

    let entry = &collector.finish()[0];
    assert!(entry.text.contains('é'));
}

/// A blank event is neither recorded nor counted as a loss: nothing was lost.
#[test]
fn blank_text_is_dropped_without_being_reported_as_truncation() {
    let mut collector = TranscriptCollector::new();
    collector.push("agent_message", "   ");

    assert!(collector.finish().is_empty());
}
