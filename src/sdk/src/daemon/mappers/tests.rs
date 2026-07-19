//! Unit tests for the JSONL line mappers: the token-usage scan, the per-provider
//! folds (claude/codex/opencode), the codex dedupe, the shared tool helpers, and
//! the RFC3339 timestamp parser.

use serde_json::Value;

use crate::tinyplace::TokenUsage;

use super::shared::{
    bound_tool_input, normalize_tool_kind, tool_display, truncate, ELISION, INPUT_CAP, OUTPUT_CAP,
};
use super::timestamp::{parse_iso_to_ms, parse_timestamp_ms};
use super::types::{HarnessLineMapper, HarnessSemanticEvent};
use super::usage::scan_usage;

#[test]
fn scan_usage_finds_nested_counts_in_all_shapes() {
    // claude: usage on an assistant/result record.
    let v: Value = serde_json::from_str(
        r#"{"type":"result","result":"ok","usage":{"input_tokens":10,"output_tokens":2}}"#,
    )
    .unwrap();
    assert_eq!(
        scan_usage(&v, 0),
        Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 2
        })
    );
    // codex: token_count info nesting.
    let v: Value = serde_json::from_str(
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":7,"output_tokens":3}}}}"#,
    )
    .unwrap();
    assert_eq!(
        scan_usage(&v, 0),
        Some(TokenUsage {
            input_tokens: 7,
            output_tokens: 3
        })
    );
    // camelCase variant.
    let v: Value = serde_json::from_str(r#"{"usage":{"inputTokens":1,"outputTokens":9}}"#).unwrap();
    assert_eq!(
        scan_usage(&v, 0),
        Some(TokenUsage {
            input_tokens: 1,
            output_tokens: 9
        })
    );
    // opencode: nested `tokens: { input, output, … }` on a message part.
    let v: Value = serde_json::from_str(
        r#"{"type":"part","part":{"tokens":{"input":12,"output":4,"reasoning":0,"cache":{"write":0,"read":0}}}}"#,
    )
    .unwrap();
    assert_eq!(
        scan_usage(&v, 0),
        Some(TokenUsage {
            input_tokens: 12,
            output_tokens: 4
        })
    );
    // No counts → None; one-sided → None.
    let v: Value = serde_json::from_str(r#"{"usage":{"input_tokens":1}}"#).unwrap();
    assert_eq!(scan_usage(&v, 0), None);
    let v: Value = serde_json::from_str(r#"{"tokens":{"input":1}}"#).unwrap();
    assert_eq!(scan_usage(&v, 0), None);
}

#[test]
fn mapper_accumulates_latest_usage() {
    let mut mapper = HarnessLineMapper::new("codex");
    assert_eq!(mapper.usage(), None);
    let _ = mapper.map_line(
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":1}}}}"#,
        0,
    );
    let _ = mapper.map_line(
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"output_tokens":11}}}}"#,
        1,
    );
    assert_eq!(
        mapper.usage(),
        Some(TokenUsage {
            input_tokens: 50,
            output_tokens: 11
        })
    );
}

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
fn claude_user_prompt_and_tool_use_and_result() {
    let user = r#"{"type":"user","timestamp":"2026-07-05T00:00:00Z","message":{"role":"user","content":"do the thing"}}"#;
    let assistant = r#"{"type":"assistant","timestamp":"2026-07-05T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"on it"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}"#;
    let result = r#"{"type":"user","timestamp":"2026-07-05T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"file1\nfile2"}]}}"#;

    let events = map_all("claude", &[user, assistant, result]);
    assert_eq!(kind_of(&events[0]), "user_prompt");
    assert_eq!(events[0].event.payload["text"], "do the thing");
    assert_eq!(events[0].event.role, "owner");
    assert_eq!(kind_of(&events[1]), "agent_message");
    assert_eq!(kind_of(&events[2]), "tool_call");
    assert_eq!(events[2].event.payload["tool_kind"], "shell");
    assert_eq!(events[2].event.payload["display"], "ls -la");
    assert_eq!(events[2].event.payload["call_id"], "t1");
    assert_eq!(kind_of(&events[3]), "tool_result");
    assert_eq!(events[3].event.payload["ok"], true);
    assert_eq!(events[3].event.payload["output"], "file1\nfile2");
    assert_eq!(events[3].event.payload["call_id"], "t1");
}

#[test]
fn codex_dedupes_double_recorded_agent_message() {
    let event_msg = r#"{"type":"event_msg","timestamp":"2026-07-05T00:00:00.000Z","payload":{"type":"agent_message","message":"final answer"}}"#;
    let response_item = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:00.500Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"final answer"}]}}"#;
    let events = map_all("codex", &[event_msg, response_item]);
    let messages: Vec<_> = events
        .iter()
        .filter(|e| kind_of(e) == "agent_message")
        .collect();
    assert_eq!(
        messages.len(),
        1,
        "duplicate agent_message should be dropped"
    );
    assert_eq!(messages[0].event.payload["text"], "final answer");
}

#[test]
fn codex_function_call_and_output_and_status() {
    let call = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:00Z","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":\"npm test\"}"}}"#;
    let output = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:01Z","payload":{"type":"function_call_output","call_id":"c1","output":"ok","success":true}}"#;
    let started = r#"{"type":"event_msg","timestamp":"2026-07-05T00:00:02Z","payload":{"type":"task_started"}}"#;
    let events = map_all("codex", &[call, output, started]);
    assert_eq!(kind_of(&events[0]), "tool_call");
    assert_eq!(events[0].event.payload["display"], "npm test");
    assert_eq!(events[0].event.payload["tool_kind"], "shell");
    assert_eq!(kind_of(&events[1]), "tool_result");
    assert_eq!(events[1].event.payload["ok"], true);
    assert_eq!(kind_of(&events[2]), "status");
    assert_eq!(events[2].event.payload["state"], "running");
    assert_eq!(events[2].event.payload["detail"], "working");
}

#[test]
fn codex_marks_error_result() {
    let output = r#"{"type":"response_item","timestamp":"2026-07-05T00:00:01Z","payload":{"type":"function_call_output","call_id":"c1","output":"boom","success":false}}"#;
    let events = map_all("codex", &[output]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["is_error"], true);
    assert_eq!(events[0].event.payload["ok"], false);
}

#[test]
fn opencode_flat_text_tool_and_error() {
    let text = r#"{"type":"text","part":{"type":"text","text":"working on it"}}"#;
    let tool_call = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r1","state":{"status":"running","input":{"file_path":"/a/b.rs"}}}}"#;
    let tool_result = r#"{"type":"tool","part":{"type":"tool","tool":"read","callID":"r1","state":{"status":"completed","output":"contents"}}}"#;
    let error =
        r#"{"type":"error","error":{"name":"ProviderError","data":{"message":"no creds"}}}"#;
    let events = map_all("opencode", &[text, tool_call, tool_result, error]);
    assert_eq!(kind_of(&events[0]), "agent_message");
    assert_eq!(events[0].event.payload["text"], "working on it");
    assert_eq!(kind_of(&events[1]), "tool_call");
    assert_eq!(events[1].event.payload["tool_kind"], "file_read");
    assert_eq!(events[1].event.payload["display"], "/a/b.rs");
    assert_eq!(kind_of(&events[2]), "tool_result");
    assert_eq!(events[2].event.payload["output"], "contents");
    assert_eq!(kind_of(&events[3]), "error");
    assert_eq!(
        events[3].event.payload["message"],
        "ProviderError: no creds"
    );
}

#[test]
fn normalize_tool_kind_ladder() {
    assert_eq!(normalize_tool_kind("mcp__github__list"), "mcp");
    assert_eq!(normalize_tool_kind("Bash"), "shell");
    assert_eq!(normalize_tool_kind("MultiEdit"), "edit");
    assert_eq!(normalize_tool_kind("Write"), "file_write");
    assert_eq!(normalize_tool_kind("Read"), "file_read");
    assert_eq!(normalize_tool_kind("Grep"), "search");
    assert_eq!(normalize_tool_kind("WebFetch"), "web");
    assert_eq!(normalize_tool_kind("Task"), "task");
    assert_eq!(normalize_tool_kind("Xyzzy"), "other");
}

#[test]
fn truncate_caps_on_byte_boundary() {
    let big = "x".repeat(OUTPUT_CAP + 100);
    let out = truncate(&big);
    assert!(out.ends_with(ELISION));
    assert!(out.len() <= OUTPUT_CAP + ELISION.len());
}

#[test]
fn malformed_and_empty_lines_yield_nothing() {
    assert!(map_all("claude", &["not json"]).is_empty());
    assert!(map_all("claude", &["[1,2,3]"]).is_empty()); // not an object
    assert!(map_all("codex", &[r#"{"type":"event_msg"}"#]).is_empty()); // no payload
    assert!(map_all("opencode", &[r#"{"type":"text"}"#]).is_empty()); // no part
                                                                      // An unknown provider maps to opencode semantics but unknown records drop.
    assert!(map_all("mystery", &[r#"{"type":"other","part":{}}"#]).is_empty());
}

#[test]
fn claude_empty_text_and_thinking_fallback() {
    // Empty assistant text produces no event.
    let empty = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#;
    assert!(map_all("claude", &[empty]).is_empty());

    // A thinking block falling back to the `text` field.
    let thinking = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","text":"pondering"}]}}"#;
    let events = map_all("claude", &[thinking]);
    assert_eq!(kind_of(&events[0]), "agent_thinking");
    assert_eq!(events[0].event.payload["text"], "pondering");

    // A user string prompt (non-array content) and the empty-string case.
    let empty_prompt = r#"{"type":"user","message":{"role":"user","content":""}}"#;
    assert!(map_all("claude", &[empty_prompt]).is_empty());

    // A tool_result with structured array content is flattened + joined.
    let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t9","is_error":true,"content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}]}}"#;
    let events = map_all("claude", &[result]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["is_error"], true);
    assert_eq!(events[0].event.payload["output"], "line1\nline2");
}

#[test]
fn codex_reasoning_mcp_and_search_variants() {
    // Reasoning falling back from summary to content.
    let reasoning = r#"{"type":"response_item","payload":{"type":"reasoning","content":[{"type":"reasoning_text","text":"deep thought"}]}}"#;
    let events = map_all("codex", &[reasoning]);
    assert_eq!(kind_of(&events[0]), "agent_thinking");
    assert_eq!(events[0].event.payload["text"], "deep thought");

    // MCP tool begin overrides tool_kind to "mcp".
    let mcp_begin = r#"{"type":"response_item","payload":{"type":"mcp_tool_call_begin","tool":"lookup","call_id":"m1","arguments":"{\"q\":\"x\"}"}}"#;
    let events = map_all("codex", &[mcp_begin]);
    assert_eq!(kind_of(&events[0]), "tool_call");
    assert_eq!(events[0].event.payload["tool_kind"], "mcp");

    // MCP tool end with a nested error output.
    let mcp_end = r#"{"type":"response_item","payload":{"type":"mcp_tool_call_end","call_id":"m1","output":{"is_error":true,"content":"boom"}}}"#;
    let events = map_all("codex", &[mcp_end]);
    assert_eq!(kind_of(&events[0]), "tool_result");
    assert_eq!(events[0].event.payload["is_error"], true);
    assert_eq!(events[0].event.payload["output"], "boom");

    // A tool_search_call driven off the `query` field.
    let search =
        r#"{"type":"response_item","payload":{"type":"tool_search_call","query":"ripgrep foo"}}"#;
    let events = map_all("codex", &[search]);
    assert_eq!(kind_of(&events[0]), "tool_call");
    assert_eq!(events[0].event.payload["tool_name"], "ripgrep foo");

    // task_complete → idle status.
    let complete = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
    let events = map_all("codex", &[complete]);
    assert_eq!(kind_of(&events[0]), "status");
    assert_eq!(events[0].event.payload["state"], "idle");
}

#[test]
fn opencode_reasoning_error_ref_and_running_tool() {
    let reasoning = r#"{"type":"reasoning","part":{"type":"reasoning","text":"  thinking  "}}"#;
    let events = map_all("opencode", &[reasoning]);
    assert_eq!(kind_of(&events[0]), "agent_thinking");
    assert_eq!(events[0].event.payload["text"], "thinking");

    // An error with a name + ref suffix.
    let error =
        r#"{"type":"error","error":{"name":"AuthError","data":{"message":"denied","ref":"E42"}}}"#;
    let events = map_all("opencode", &[error]);
    assert_eq!(
        events[0].event.payload["message"],
        "AuthError: denied (E42)"
    );

    // A running (non-terminal) tool with no output → a tool_call, not result.
    let running = r#"{"type":"tool","part":{"type":"tool","tool":"bash","callID":"b1","state":{"status":"running","input":{"command":"ls"}}}}"#;
    let events = map_all("opencode", &[running]);
    assert_eq!(kind_of(&events[0]), "tool_call");
    assert_eq!(events[0].event.payload["tool_kind"], "shell");
}

#[test]
fn tool_display_and_bound_input_helpers() {
    // Falls back to the tool name when no known key is present.
    assert_eq!(tool_display("Weird", &serde_json::json!({})), "Weird");
    // A bare string input is used directly.
    assert_eq!(
        tool_display("X", &serde_json::json!("just a string")),
        "just a string"
    );
    // Oversized structured input collapses to a byte-capped string.
    let big = "y".repeat(INPUT_CAP + 50);
    let bounded = bound_tool_input(&serde_json::json!({ "blob": big }));
    assert!(bounded.is_string());
    assert!(bounded.as_str().unwrap().ends_with(ELISION));
}

#[test]
fn parse_iso_rejects_garbage() {
    assert!(parse_iso_to_ms("nope").is_none());
    assert!(parse_iso_to_ms("2026/07/05").is_none());
    // A receive-time fallback is used when the field is absent (non-panicking).
    let ts = parse_timestamp_ms(None);
    assert!(ts > 0);
}

#[test]
fn parses_iso_timestamp() {
    // 2026-07-05T00:00:00Z is 1_783_209_600 s since the Unix epoch.
    let ms = parse_iso_to_ms("2026-07-05T00:00:00Z").unwrap();
    assert_eq!(ms, 1_783_209_600_000);
    let with_ms = parse_iso_to_ms("2026-07-05T00:00:00.500Z").unwrap();
    assert_eq!(with_ms, 1_783_209_600_500);
    // Offset handling: 01:00+01:00 is the same instant as 00:00Z.
    let offset = parse_iso_to_ms("2026-07-05T01:00:00+01:00").unwrap();
    assert_eq!(offset, 1_783_209_600_000);
}
