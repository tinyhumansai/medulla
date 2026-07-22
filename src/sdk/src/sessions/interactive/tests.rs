//! Unit tests for the interactive stream protocol: the frame fold and the
//! stdin encoders.
//!
//! The process itself is not exercised here — that needs a real `claude` binary
//! and lives in the crate's e2e suite. What is pinned here is every JSON shape
//! the protocol depends on, because a silent change in one produces a session
//! that hangs rather than one that errors.

use serde_json::json;

use super::frames::{encode_interrupt, encode_user_message, map_stream_frame, StreamEvent};
use super::session::{build_interactive_args, InteractiveSpec};
use crate::tinyplace::HarnessProvider;

#[test]
fn a_system_init_frame_announces_the_session_id() {
    let frame = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "abc-123",
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Session {
            session_id: "abc-123".to_string()
        }]
    );
}

#[test]
fn a_non_init_system_frame_announces_nothing() {
    let frame = json!({ "type": "system", "subtype": "other", "session_id": "abc" });
    assert!(map_stream_frame(&frame).is_empty());
}

#[test]
fn a_result_frame_is_the_turn_terminator() {
    // The single most load-bearing shape in the module: completion is this
    // frame, not process exit.
    let frame = json!({
        "type": "result",
        "result": "all done",
        "is_error": false,
        "session_id": "abc-123",
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Result {
            reply: "all done".to_string(),
            is_error: false,
            session_id: Some("abc-123".to_string()),
        }]
    );
}

#[test]
fn an_interrupted_result_frame_still_terminates_the_turn() {
    // What the CLI emits after an in-band interrupt: an error-flagged result
    // with no text. It must still fold to a terminator, or the turn hangs.
    let frame = json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Result {
            reply: String::new(),
            is_error: true,
            session_id: None,
        }]
    );
}

#[test]
fn one_assistant_frame_can_carry_several_blocks() {
    let frame = json!({
        "type": "assistant",
        "message": { "content": [
            { "type": "thinking", "thinking": "hmm" },
            { "type": "text", "text": "here" },
            { "type": "tool_use", "name": "Read", "input": { "path": "a.rs" } },
        ]},
    });
    let events = map_stream_frame(&frame);
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        StreamEvent::ReasoningDelta {
            text: "hmm".to_string()
        }
    );
    assert_eq!(
        events[1],
        StreamEvent::AssistantDelta {
            text: "here".to_string()
        }
    );
    assert!(
        matches!(&events[2], StreamEvent::Tool { label } if label.starts_with("Read · ")),
        "got {:?}",
        events[2]
    );
}

#[test]
fn empty_blocks_and_unknown_frame_types_fold_to_nothing() {
    // The stream is an open vocabulary: user echoes and control responses must
    // pass through silently rather than erroring.
    assert!(map_stream_frame(&json!({ "type": "user" })).is_empty());
    assert!(map_stream_frame(&json!({ "type": "control_response" })).is_empty());
    assert!(map_stream_frame(&json!({ "no_type": true })).is_empty());
    assert!(map_stream_frame(&json!({
        "type": "assistant",
        "message": { "content": [{ "type": "text", "text": "" }] },
    }))
    .is_empty());
}

#[test]
fn a_long_tool_input_is_clipped() {
    let frame = json!({
        "type": "assistant",
        "message": { "content": [
            { "type": "tool_use", "name": "Write", "input": { "body": "x".repeat(500) } },
        ]},
    });
    let events = map_stream_frame(&frame);
    let StreamEvent::Tool { label } = &events[0] else {
        panic!("expected a tool event");
    };
    assert!(label.ends_with('…'), "long input must be clipped: {label}");
    assert!(label.chars().count() < 300);
}

#[test]
fn a_turn_is_encoded_as_one_newline_terminated_user_message() {
    let line = encode_user_message("hello");
    assert!(
        line.ends_with('\n'),
        "the CLI reads one JSON object per line"
    );
    assert_eq!(line.matches('\n').count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"][0]["text"], "hello");
}

#[test]
fn a_multiline_prompt_stays_one_wire_line() {
    // A raw newline in the prompt would split the frame and desync the stream.
    let line = encode_user_message("first\nsecond");
    assert_eq!(line.matches('\n').count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["message"]["content"][0]["text"], "first\nsecond");
}

#[test]
fn an_interrupt_is_encoded_as_a_control_request() {
    let line = encode_interrupt("req_interrupt_1");
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request_id"], "req_interrupt_1");
    assert_eq!(parsed["request"]["subtype"], "interrupt");
}

fn spec() -> InteractiveSpec {
    InteractiveSpec {
        provider: HarnessProvider::Claude,
        bin: "claude".to_string(),
        cwd: "/repo".to_string(),
        env: std::collections::HashMap::new(),
        model: None,
        append_system_prompt: None,
        skip_permissions: false,
        extra_args: Vec::new(),
    }
}

#[test]
fn the_interactive_argv_asks_for_a_stream_json_channel_in_both_directions() {
    let args = build_interactive_args(&spec());
    assert_eq!(
        args,
        vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose"
        ]
    );
}

#[test]
fn the_interactive_argv_carries_no_prompt() {
    // Turns arrive on stdin, which is what sidesteps the leading-dash injection
    // hazard the one-shot argv has to neutralize.
    let mut spec = spec();
    spec.model = Some("opus".to_string());
    spec.skip_permissions = true;
    spec.extra_args = vec!["--foo".to_string()];
    let args = build_interactive_args(&spec);

    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"opus".to_string()));
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(args.last().unwrap(), "--foo");
    assert!(
        !args.iter().any(|arg| arg == "-"),
        "no positional prompt belongs on an interactive argv"
    );
}
