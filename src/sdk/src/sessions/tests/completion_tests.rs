//! Tests for interactive turn-completion detection.
//!
//! The fixtures mirror the record shape of real `~/.claude/projects/**` JSONL
//! (verified against 575 local transcripts); only the text is synthetic.

use super::super::completion::{TurnSignal, TurnWatcher};

/// The record that follows a finished message in a real transcript — `system`,
/// then `last-prompt` / `ai-title`. Anything not carrying the message's own id
/// closes it.
const AFTER_MESSAGE: &str = r#"{"type":"system","subtype":"post_turn"}"#;

/// An `assistant` record with the given content blocks and stop reason.
fn assistant(blocks: serde_json::Value, stop_reason: Option<&str>) -> String {
    assistant_msg(blocks, stop_reason, "m-1")
}

/// An `assistant` record belonging to message `id`.
///
/// Claude Code writes one record per content block and repeats `message.id` on
/// each, so the id is what ties a split message back together.
fn assistant_msg(blocks: serde_json::Value, stop_reason: Option<&str>, id: &str) -> String {
    let mut message = serde_json::json!({
        "role": "assistant",
        "type": "message",
        "id": id,
        "content": blocks,
    });
    if let Some(reason) = stop_reason {
        message["stop_reason"] = serde_json::json!(reason);
    }
    serde_json::json!({
        "type": "assistant",
        "isSidechain": false,
        "sessionId": "s-1",
        "uuid": "u-1",
        "message": message,
    })
    .to_string()
}

fn text_block(text: &str) -> serde_json::Value {
    serde_json::json!([{ "type": "text", "text": text }])
}

fn tool_block(name: &str) -> serde_json::Value {
    serde_json::json!([{ "type": "tool_use", "name": name, "input": {} }])
}

#[test]
fn end_turn_completes_the_turn_with_the_assistants_answer() {
    // The whole point: completion is *stated* by the transcript, not inferred.
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("all done"), Some("end_turn")));
    assert_eq!(
        watcher.observe(AFTER_MESSAGE),
        Some(TurnSignal::Complete {
            reply: "all done".to_string(),
            stop_reason: "end_turn".to_string(),
        })
    );
    assert!(watcher.is_done());
}

#[test]
fn a_terminal_message_split_across_records_replies_with_its_text() {
    // Claude Code writes ONE RECORD PER CONTENT BLOCK, repeating the
    // message-level stop_reason on every one. A final `[thinking, text]` message
    // is therefore two `end_turn` records, thinking first — and thinking is
    // deliberately not part of a reply.
    //
    // Settling on the first one shipped the narration that happened to precede
    // it, or nothing at all. Observed in the field: a worker asked to run nine
    // commands answered "I'll survey the workspace. Let me start with the
    // basics." — and another answered with zero characters.
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("Let me start."), Some("tool_use")));
    watcher.observe(&assistant(tool_block("Bash"), Some("tool_use")));
    let thinking = serde_json::json!([{ "type": "thinking", "thinking": "that covers it" }]);
    assert!(
        !matches!(
            watcher.observe(&assistant(thinking, Some("end_turn"))),
            Some(TurnSignal::Complete { .. })
        ),
        "the thinking block of a terminal message must not settle the turn — \
         the answer is in the record after it"
    );
    watcher.observe(&assistant(
        text_block("Here is the output."),
        Some("end_turn"),
    ));

    let signal = watcher.observe(AFTER_MESSAGE);
    let Some(TurnSignal::Complete { reply, .. }) = signal else {
        panic!("expected completion, got {signal:?}");
    };
    assert_eq!(reply, "Let me start.\nHere is the output.");
}

#[test]
fn a_pending_terminal_settles_without_a_following_record() {
    // A transcript that simply stops must not hold a finished turn for the full
    // stall budget; the executor closes it after a short grace.
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("the answer"), Some("end_turn")));
    assert!(watcher.terminal_pending());
    assert!(!watcher.is_done(), "not settled until the message closes");

    let signal = watcher.settle_pending();
    assert!(
        matches!(signal, Some(TurnSignal::Complete { ref reply, .. }) if reply == "the answer"),
        "got {signal:?}"
    );
    assert!(watcher.is_done());
    assert!(watcher.settle_pending().is_none(), "settles exactly once");
}

#[test]
fn a_new_messages_records_never_join_the_pending_reply() {
    // Closing on "not this message id" must mean exactly that: the next turn's
    // text is not ours, even though it is also an assistant record.
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant_msg(
        text_block("ours"),
        Some("end_turn"),
        "m-first",
    ));
    let signal = watcher.observe(&assistant_msg(
        text_block("someone else's"),
        Some("end_turn"),
        "m-second",
    ));
    assert!(
        matches!(signal, Some(TurnSignal::Complete { ref reply, .. }) if reply == "ours"),
        "got {signal:?}"
    );
}

#[test]
fn tool_use_does_not_complete_the_turn() {
    // 54,837 of ~60k observed records are tool_use. Treating any of them as
    // terminal would end almost every turn on its first tool call.
    let mut watcher = TurnWatcher::new();
    let signal = watcher.observe(&assistant(tool_block("Bash"), Some("tool_use")));
    assert_eq!(
        signal,
        Some(TurnSignal::Tool {
            name: "Bash".to_string()
        })
    );
    assert!(!watcher.is_done());
    assert!(watcher.tool_outstanding());
}

#[test]
fn stop_sequence_is_terminal_too() {
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("halted"), Some("stop_sequence")));
    let signal = watcher.observe(AFTER_MESSAGE);
    assert!(
        matches!(signal, Some(TurnSignal::Complete { ref stop_reason, .. }) if stop_reason == "stop_sequence"),
        "got {signal:?}"
    );
    assert!(watcher.is_done());
}

#[test]
fn a_multi_step_turn_accumulates_every_answer_then_settles_once() {
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("looking now"), Some("tool_use")));
    watcher.observe(&assistant(tool_block("Read"), Some("tool_use")));
    watcher.observe(&assistant(text_block("found it"), Some("tool_use")));
    watcher.observe(&assistant(
        text_block("fixed in client.rs"),
        Some("end_turn"),
    ));
    let signal = watcher.observe(AFTER_MESSAGE);

    let Some(TurnSignal::Complete { reply, .. }) = signal else {
        panic!("expected completion, got {signal:?}");
    };
    assert_eq!(reply, "looking now\nfound it\nfixed in client.rs");
}

#[test]
fn thinking_is_not_part_of_the_reply() {
    // Scratch work is not the answer the peer asked for.
    let mut watcher = TurnWatcher::new();
    let blocks = serde_json::json!([
        { "type": "thinking", "thinking": "let me consider the options" },
        { "type": "text", "text": "the answer" },
    ]);
    watcher.observe(&assistant(blocks, Some("end_turn")));
    let signal = watcher.observe(AFTER_MESSAGE);
    let Some(TurnSignal::Complete { reply, .. }) = signal else {
        panic!("expected completion");
    };
    assert_eq!(reply, "the answer");
}

#[test]
fn nothing_is_reported_after_the_turn_has_settled() {
    // The next turn's records must not leak into this one's reply.
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant_msg(text_block("done"), Some("end_turn"), "m-1"));
    watcher.observe(AFTER_MESSAGE);
    assert!(watcher.is_done());
    assert_eq!(
        watcher.observe(&assistant_msg(
            text_block("next turn"),
            Some("end_turn"),
            "m-2"
        )),
        None
    );
    assert_eq!(watcher.reply(), "done");
}

#[test]
fn non_assistant_and_malformed_lines_say_nothing() {
    let mut watcher = TurnWatcher::new();
    for line in [
        r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"file-history-snapshot"}"#,
        r#"{"type":"last-prompt","lastPrompt":"x"}"#,
        "not json at all",
        "",
    ] {
        assert_eq!(watcher.observe(line), None, "line: {line}");
    }
    assert!(!watcher.is_done());
}

#[test]
fn a_sidechain_record_can_never_settle_our_turn() {
    // Sub-agents write their own transcript, so this should not arise — but a
    // sub-agent's end_turn settling the parent is the exact truncation trap the
    // hook-based approach falls into, so it is guarded rather than assumed.
    let mut watcher = TurnWatcher::new();
    let mut record: serde_json::Value =
        serde_json::from_str(&assistant(text_block("subagent done"), Some("end_turn"))).unwrap();
    record["isSidechain"] = serde_json::json!(true);
    assert_eq!(watcher.observe(&record.to_string()), None);
    assert!(!watcher.is_done());
}

#[test]
fn an_absent_stop_reason_is_not_treated_as_terminal() {
    // 0.08% of observed records carry none. Guessing "finished" would truncate
    // a live turn; guessing "working" is recoverable via the stall backstop.
    let mut watcher = TurnWatcher::new();
    let signal = watcher.observe(&assistant(text_block("mid sentence"), None));
    assert_eq!(
        signal,
        Some(TurnSignal::Progress {
            text: "mid sentence".to_string()
        })
    );
    assert!(!watcher.is_done());
}

#[test]
fn the_stall_backstop_waits_for_the_budget() {
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("hmm"), None));
    assert!(!watcher.stalled_for(4_000, 10_000), "under budget");
    assert!(watcher.stalled_for(10_000, 10_000), "at budget");

    let signal = watcher.settle_stalled();
    assert!(
        matches!(signal, TurnSignal::Complete { ref stop_reason, .. } if stop_reason == "stalled"),
        "got {signal:?}"
    );
    assert!(watcher.is_done());
}

#[test]
fn the_stall_backstop_refuses_while_a_tool_is_outstanding() {
    // A four-minute build is silence that means the opposite of "finished".
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(tool_block("Bash"), Some("tool_use")));
    assert!(
        !watcher.stalled_for(600_000, 10_000),
        "a running tool must never be settled as a stall"
    );
}

#[test]
fn a_settled_turn_never_stalls() {
    let mut watcher = TurnWatcher::new();
    watcher.observe(&assistant(text_block("done"), Some("end_turn")));
    watcher.observe(AFTER_MESSAGE);
    assert!(!watcher.stalled_for(999_999, 10_000));
}

// ------------------------------------------------------------------ codex ---
//
// Shapes taken from real `~/.codex/sessions/**` rollouts; text is synthetic.

use crate::tinyplace::HarnessProvider;

/// A codex `event_msg` rollout record.
fn event_msg(payload: serde_json::Value) -> String {
    serde_json::json!({
        "type": "event_msg",
        "timestamp": "2026-07-21T00:00:00.000Z",
        "payload": payload,
    })
    .to_string()
}

fn codex_watcher() -> TurnWatcher {
    TurnWatcher::for_provider(HarnessProvider::Codex)
}

#[test]
fn codex_task_complete_ends_the_turn_and_carries_the_answer() {
    // `last_agent_message` is on the completion event itself, so nothing has to
    // be accumulated across records.
    let mut watcher = codex_watcher();
    watcher.observe(&event_msg(
        serde_json::json!({ "type": "task_started", "turn_id": "t1" }),
    ));
    let signal = watcher.observe(&event_msg(serde_json::json!({
        "type": "task_complete",
        "turn_id": "t1",
        "last_agent_message": "patched the retry path",
        "duration_ms": 4200,
    })));
    assert_eq!(
        signal,
        Some(TurnSignal::Complete {
            reply: "patched the retry path".to_string(),
            stop_reason: "task_complete".to_string(),
        })
    );
    assert!(watcher.is_done());
}

#[test]
fn a_running_codex_turn_is_never_settled_by_the_stall_backstop() {
    // `task_started` with no completion yet is work in progress, not silence.
    let mut watcher = codex_watcher();
    watcher.observe(&event_msg(
        serde_json::json!({ "type": "task_started", "turn_id": "t1" }),
    ));
    assert!(!watcher.is_done());
    assert!(
        !watcher.stalled_for(600_000, 10_000),
        "a started-but-unfinished turn must not be settled as a stall"
    );
}

#[test]
fn codex_agent_messages_stream_as_progress() {
    let mut watcher = codex_watcher();
    let signal = watcher.observe(&event_msg(serde_json::json!({
        "type": "agent_message",
        "message": "looking at the client",
        "phase": "main",
    })));
    assert_eq!(
        signal,
        Some(TurnSignal::Progress {
            text: "looking at the client".to_string()
        })
    );
    assert!(!watcher.is_done());
}

#[test]
fn codex_falls_back_to_streamed_text_when_the_completion_carries_none() {
    let mut watcher = codex_watcher();
    watcher.observe(&event_msg(serde_json::json!({
        "type": "agent_message", "message": "the answer", "phase": "main",
    })));
    let signal = watcher.observe(&event_msg(serde_json::json!({
        "type": "task_complete", "turn_id": "t1", "last_agent_message": "",
    })));
    let Some(TurnSignal::Complete { reply, .. }) = signal else {
        panic!("expected completion, got {signal:?}");
    };
    assert_eq!(reply, "the answer");
}

#[test]
fn codex_turn_aborted_settles_the_turn_with_its_reason() {
    let mut watcher = codex_watcher();
    watcher.observe(&event_msg(serde_json::json!({
        "type": "agent_message", "message": "partial work", "phase": "main",
    })));
    let signal = watcher.observe(&event_msg(serde_json::json!({
        "type": "turn_aborted", "turn_id": "t1", "reason": "interrupted",
    })));
    let Some(TurnSignal::Complete { reply, stop_reason }) = signal else {
        panic!("expected completion, got {signal:?}");
    };
    assert_eq!(
        reply, "partial work",
        "an abort still returns what was said"
    );
    assert_eq!(stop_reason, "aborted: interrupted");
}

#[test]
fn codex_ignores_records_that_are_not_turn_events() {
    let mut watcher = codex_watcher();
    for line in [
        r#"{"type":"session_meta","payload":{"session_id":"s","cwd":"/repo"}}"#,
        r#"{"type":"response_item","payload":{"type":"reasoning"}}"#,
        r#"{"type":"turn_context","payload":{"turn_id":"t1","cwd":"/repo"}}"#,
        &event_msg(serde_json::json!({ "type": "token_count", "total": 12 })),
    ] {
        assert_eq!(watcher.observe(line), None, "line: {line}");
    }
    assert!(!watcher.is_done());
}

#[test]
fn a_claude_watcher_does_not_read_codex_records_or_the_reverse() {
    // The dialects are disjoint; folding one with the other's rules would
    // silently never complete.
    let mut claude = TurnWatcher::new();
    assert_eq!(
        claude.observe(&event_msg(serde_json::json!({
            "type": "task_complete", "turn_id": "t1", "last_agent_message": "x",
        }))),
        None
    );

    let mut codex = codex_watcher();
    assert_eq!(
        codex.observe(&assistant(text_block("x"), Some("end_turn"))),
        None
    );
}

#[test]
fn opencode_has_no_rollout_to_read() {
    // Which is exactly why it is not offered as a task target.
    let mut watcher = TurnWatcher::for_provider(HarnessProvider::Opencode);
    assert_eq!(
        watcher.observe(&assistant(text_block("x"), Some("end_turn"))),
        None
    );
    assert!(!watcher.is_done());
}
