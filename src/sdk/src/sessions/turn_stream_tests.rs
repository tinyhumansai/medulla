//! Unit tests for [`TurnStream`], the mode-independent fold.
//!
//! The stream drives two collaborators off each line — the semantic-event mapper
//! and the completion watcher — and exposes their combined state. These tests
//! pin the contract the two harness modes both depend on: a blank line is inert,
//! a tool call is progress rather than an ending, a stated terminal is held until
//! its message closes, and a turn with no stated reason still settles through the
//! stall backstop.

use super::turn_stream::TurnStream;
use crate::tinyplace::HarnessProvider;

/// A claude assistant record that invokes a tool and states it will continue.
const TOOL_LINE: &str = r#"{"type":"assistant","message":{"role":"assistant","id":"m1","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#;

/// A claude assistant record that ends the turn, carrying the reply text.
const END_LINE: &str = r#"{"type":"assistant","message":{"role":"assistant","id":"m2","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#;

/// A following record from a different message — closes a held terminal.
const CLOSER_LINE: &str = r#"{"type":"user","message":{"role":"user","content":"next"}}"#;

/// A claude assistant record with text but no stated stop_reason.
const NO_REASON_LINE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"partial"}]}}"#;

#[test]
fn a_blank_line_folds_to_nothing_and_does_not_advance_the_stream() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    let fold = stream.observe("   ");
    assert!(fold.events.is_empty(), "a blank line carries no events");
    assert!(fold.reply.is_none());
    assert!(!fold.is_complete());
    assert_eq!(stream.events(), 0, "the event count is untouched");
    assert!(!stream.is_done());
}

#[test]
fn a_tool_call_is_surfaced_as_progress_but_does_not_end_the_turn() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    let fold = stream.observe(TOOL_LINE);

    assert!(
        !fold.events.is_empty(),
        "the tool call reaches the peer as a semantic event"
    );
    assert!(fold.reply.is_none(), "a tool call is not a completion");
    assert!(!fold.is_complete());
    assert!(!stream.is_done());
    assert!(
        stream.tool_outstanding(),
        "the watcher records the outstanding tool call"
    );
    assert_eq!(
        stream.events(),
        fold.events.len(),
        "the running total matches what this line produced"
    );
}

#[test]
fn an_end_turn_record_is_held_until_its_message_closes_then_yields_the_reply() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);

    // The terminal record alone does not complete the turn: the reply may still
    // be arriving in later blocks of the same message.
    let held = stream.observe(END_LINE);
    assert!(held.reply.is_none(), "terminal held, not yet settled");
    assert!(stream.terminal_pending());
    assert!(!stream.is_done());

    // A record from a different message closes it.
    let done = stream.observe(CLOSER_LINE);
    assert_eq!(
        done.reply.as_deref(),
        Some("done"),
        "the reply is delivered"
    );
    assert!(done.is_complete());
    assert!(stream.is_done());
}

#[test]
fn a_held_terminal_can_be_settled_directly_when_nothing_follows() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    stream.observe(END_LINE);
    assert!(stream.terminal_pending());

    assert_eq!(
        stream.settle_pending().as_deref(),
        Some("done"),
        "settling a held terminal returns its reply"
    );
    assert!(stream.is_done());

    // With nothing pending there is nothing to settle.
    let mut fresh = TurnStream::new(HarnessProvider::Claude);
    assert!(fresh.settle_pending().is_none());
}

#[test]
fn usage_reflects_the_latest_counts_the_harness_reported() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    assert!(stream.usage().is_none(), "no counts before any line");

    stream.observe(
        r#"{"type":"result","result":"ok","usage":{"input_tokens":10,"output_tokens":2}}"#,
    );
    let usage = stream.usage().expect("a result line carries usage");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 2);
}

#[test]
fn a_turn_with_no_stated_reason_settles_through_the_stall_backstop() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    stream.observe(NO_REASON_LINE);

    assert!(
        !stream.is_done(),
        "an unstated reason is not itself terminal"
    );
    assert!(!stream.tool_outstanding());
    assert!(
        !stream.stalled_for(10, 50),
        "silence shorter than the budget is not a stall"
    );
    assert!(
        stream.stalled_for(100, 50),
        "silence past the budget is a stall"
    );

    assert_eq!(
        stream.settle_stalled(),
        "partial",
        "the backstop replies with whatever was said"
    );
    assert!(stream.is_done());
}

#[test]
fn an_outstanding_tool_call_holds_off_the_stall_backstop() {
    let mut stream = TurnStream::new(HarnessProvider::Claude);
    stream.observe(TOOL_LINE);
    assert!(stream.tool_outstanding());

    assert!(
        !stream.stalled_for(10_000, 10),
        "a long-running tool is silence that means work, not a finished turn"
    );
}
