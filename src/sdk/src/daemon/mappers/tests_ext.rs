//! Additional branch-coverage tests for the JSONL line mappers: opencode/codex
//! error and tool shapes, the claude content-block folds, the shared JSON/text
//! helpers, and the RFC3339 fraction/offset edges. Split from `tests.rs` to keep
//! each test file under the module size ceiling.

use super::shared::{
    as_array, bound_tool_input, parse_json_object, parse_maybe_json, safe_stringify,
    text_from_content, tool_display, truncate, ELISION, INPUT_CAP, OUTPUT_CAP,
};
use super::timestamp::parse_iso_to_ms;
use super::types::{HarnessLineMapper, HarnessSemanticEvent};

fn map_all(provider: &str, lines: &[&str]) -> Vec<HarnessSemanticEvent> {
    let mut mapper = HarnessLineMapper::new(provider);
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        out.extend(mapper.map_line(line, index as i64));
    }
    out
}

fn kind_of(event: &HarnessSemanticEvent) -> &str {
    &event.event.kind
}

#[test]
fn opencode_error_without_message_falls_back_to_stringify() {
    // An error object with no `message`/`data.message` → the raw error is
    // stringified rather than dropped.
    let error = r#"{"type":"error","error":{"code":"boom","detail":"nope"}}"#;
    let events = map_all("opencode", &[error]);
    assert_eq!(kind_of(&events[0]), "error");
    let msg = events[0].event.payload["message"].as_str().unwrap();
    assert!(msg.contains("boom") || msg.contains("nope"), "msg: {msg}");

    // An error record with no `error` field at all stringifies the whole record.
    let bare = r#"{"type":"error","note":"kaboom"}"#;
    let events = map_all("opencode", &[bare]);
    assert_eq!(kind_of(&events[0]), "error");
    assert!(events[0].event.payload["message"]
        .as_str()
        .unwrap()
        .contains("kaboom"));
}

#[test]
fn opencode_error_uses_top_level_message_without_data() {
    // No `data.message` but a top-level `message` → "Name: message".
    let error = r#"{"type":"error","error":{"name":"BadThing","message":"it broke"}}"#;
    let events = map_all("opencode", &[error]);
    assert_eq!(events[0].event.payload["message"], "BadThing: it broke");
}

#[test]
fn opencode_empty_text_and_reasoning_drop() {
    // Whitespace-only text/reasoning parts produce no events.
    let text = r#"{"type":"text","part":{"type":"text","text":"   "}}"#;
    assert!(map_all("opencode", &[text]).is_empty());
    let reasoning = r#"{"type":"reasoning","part":{"type":"reasoning","text":"\n\t"}}"#;
    assert!(map_all("opencode", &[reasoning]).is_empty());
}

#[test]
fn opencode_tool_terminal_via_output_and_output_shapes() {
    // A non-terminal status but a present output still marks the call finished.
    let via_output = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r1","state":{"status":"running","output":"data here"}}}"#;
    let events = map_all("opencode", &[via_output]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["output"], "data here");

    // A structured (non-string) output is stringified.
    let obj_output = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r2","state":{"status":"completed","output":{"lines":3}}}}"#;
    let events = map_all("opencode", &[obj_output]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert!(events[0].event.payload["output"]
        .as_str()
        .unwrap()
        .contains("lines"));

    // A completed call with no output yields an empty output string.
    let no_output = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r3","state":{"status":"completed"}}}"#;
    let events = map_all("opencode", &[no_output]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["output"], "");
}

#[test]
fn codex_user_message_present_and_empty() {
    let present = r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi there"}}"#;
    let events = map_all("codex", &[present]);
    assert_eq!(kind_of(&events[0]), "user_prompt");
    assert_eq!(events[0].event.payload["text"], "hi there");

    let empty = r#"{"type":"event_msg","payload":{"type":"user_message","message":""}}"#;
    assert!(map_all("codex", &[empty]).is_empty());
}

#[test]
fn codex_response_item_message_via_content() {
    // A lone assistant response_item (no `message` field) folds via `content`.
    let item = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello world"}]}}"#;
    let events = map_all("codex", &[item]);
    assert_eq!(kind_of(&events[0]), "agent_message");
    assert_eq!(events[0].event.payload["text"], "hello world");

    // An assistant message with empty content drops.
    let empty =
        r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[]}}"#;
    assert!(map_all("codex", &[empty]).is_empty());
}

#[test]
fn codex_output_text_object_and_scalar_shapes() {
    // An output object whose `content` is a structured array is flattened.
    let arr_content = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":{"content":[{"type":"output_text","text":"nested"}]}}}"#;
    let events = map_all("codex", &[arr_content]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["output"], "nested");

    // A scalar (array) output falls through to the generic content extractor.
    let scalar = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":[{"type":"text","text":"listy"}]}}"#;
    let events = map_all("codex", &[scalar]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["output"], "listy");
}

#[test]
fn codex_error_flagged_via_nested_output_object() {
    // `success:false` nested inside the `output` object flags an error.
    let nested = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c3","output":{"success":false,"content":"failed"}}}"#;
    let events = map_all("codex", &[nested]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["is_error"], true);
}

#[test]
fn claude_user_content_blocks_and_unknown_records() {
    // A `system` record folds to nothing (neither user nor assistant).
    assert!(map_all("claude", &[r#"{"type":"system","message":{"role":"x"}}"#]).is_empty());

    // A user message whose content array holds a text block + a non-object block
    // + an unknown-type block.
    let user = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"},42,{"type":"image"},{"type":"text","text":""}]}}"#;
    let events = map_all("claude", &[user]);
    assert_eq!(events.len(), 1);
    assert_eq!(kind_of(&events[0]), "user_prompt");
    assert_eq!(events[0].event.payload["text"], "hello");

    // A tool_result whose content is a mix of text and non-text blocks, plus a
    // scalar content shape that flattens to empty.
    let mixed = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a"},{"type":"image"}]}]}}"#;
    let events = map_all("claude", &[mixed]);
    assert_eq!(events[0].event.payload["output"], "a");
    let scalar = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":99}]}}"#;
    let events = map_all("claude", &[scalar]);
    assert_eq!(events[0].event.payload["output"], "");
}

#[test]
fn claude_assistant_blocks_edge_cases() {
    // A non-object block and an unknown-type block are dropped; empty thinking
    // yields nothing.
    let assistant = r#"{"type":"assistant","message":{"role":"assistant","content":[7,{"type":"redacted"},{"type":"thinking","thinking":""}]}}"#;
    assert!(map_all("claude", &[assistant]).is_empty());

    // Thinking prefers the `thinking` field over `text`.
    let thinking = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"deep"}]}}"#;
    let events = map_all("claude", &[thinking]);
    assert_eq!(kind_of(&events[0]), "agent_thinking");
    assert_eq!(events[0].event.payload["text"], "deep");
}

#[test]
fn shared_tool_display_and_first_line_helpers() {
    // A known key is used for the display; a long value is capped with an ellipsis.
    assert_eq!(
        tool_display("Bash", &serde_json::json!({ "command": "npm run build" })),
        "npm run build"
    );
    let long = "z".repeat(400);
    let out = tool_display("Bash", &serde_json::json!({ "command": long }));
    assert!(out.ends_with("..."));
    assert!(out.chars().count() <= 200);
    // Only the first line is kept.
    assert_eq!(
        tool_display("Read", &serde_json::json!({ "file_path": "a.rs\nignored" })),
        "a.rs"
    );
}

#[test]
fn shared_truncate_and_bound_respect_utf8_boundaries() {
    // A 3-byte char (☃ = 3 bytes) makes the byte cap land mid-code-point, so
    // the boundary-walk-back loop runs rather than slicing cleanly.
    let multibyte = "☃".repeat(OUTPUT_CAP);
    let out = truncate(&multibyte);
    assert!(out.ends_with(ELISION));
    // The kept prefix is valid UTF-8 (would panic on a non-boundary slice).
    assert!(out.chars().count() > 0);
    let big_obj = serde_json::json!({ "k": "☃".repeat(INPUT_CAP) });
    let bounded = bound_tool_input(&big_obj);
    assert!(bounded.as_str().unwrap().ends_with(ELISION));
}

#[test]
fn shared_json_shape_helpers() {
    // safe_stringify: strings verbatim, structures as compact JSON.
    assert_eq!(safe_stringify(&serde_json::json!("hi")), "hi");
    assert_eq!(safe_stringify(&serde_json::json!({ "a": 1 })), r#"{"a":1}"#);

    // text_from_content: string, filtered array, and non-array fall-through.
    assert_eq!(
        text_from_content(Some(&serde_json::json!("plain")), &["text"]),
        "plain"
    );
    let arr = serde_json::json!([{"type":"text","text":"x"},{"type":"other","text":"y"}]);
    assert_eq!(text_from_content(Some(&arr), &["text"]), "x");
    assert_eq!(
        text_from_content(Some(&serde_json::json!(5)), &["text"]),
        ""
    );

    // parse_maybe_json: absent, non-string, plain-string, object-string, garbage.
    assert_eq!(parse_maybe_json(None), None);
    assert_eq!(
        parse_maybe_json(Some(&serde_json::json!(7))),
        Some(serde_json::json!(7))
    );
    assert_eq!(
        parse_maybe_json(Some(&serde_json::json!("plain text"))),
        Some(serde_json::json!("plain text"))
    );
    assert_eq!(
        parse_maybe_json(Some(&serde_json::json!("{\"a\":1}"))),
        Some(serde_json::json!({ "a": 1 }))
    );
    assert_eq!(
        parse_maybe_json(Some(&serde_json::json!("{not json"))),
        Some(serde_json::json!("{not json"))
    );

    // parse_json_object rejects non-objects; as_array falls back to empty.
    assert!(parse_json_object("[1,2]").is_none());
    assert!(parse_json_object("{\"k\":1}").is_some());
    assert!(as_array(Some(&serde_json::json!("x"))).is_empty());
    assert_eq!(as_array(Some(&serde_json::json!([1, 2]))).len(), 2);
}

#[test]
fn parse_iso_rejects_malformed_separators_and_handles_fractions() {
    // Each structural separator check rejects a corrupt input.
    assert!(parse_iso_to_ms("2026x07-05T00:00:00Z").is_none());
    assert!(parse_iso_to_ms("2026-07x05T00:00:00Z").is_none());
    assert!(parse_iso_to_ms("2026-07-05x00:00:00Z").is_none());
    assert!(parse_iso_to_ms("2026-07-05T00x00:00Z").is_none());
    assert!(parse_iso_to_ms("2026-07-05T00:00x00Z").is_none());

    // More than three fractional digits are truncated to milliseconds.
    let long_frac = parse_iso_to_ms("2026-07-05T00:00:00.123456Z").unwrap();
    assert_eq!(long_frac % 1000, 123);
    // Fewer than three fractional digits are padded.
    let short_frac = parse_iso_to_ms("2026-07-05T00:00:00.5Z").unwrap();
    assert_eq!(short_frac % 1000, 500);
    // A non-Z/offset trailing char is tolerated (treated like UTC).
    assert!(parse_iso_to_ms("2026-07-05T00:00:00X").is_some());
}
