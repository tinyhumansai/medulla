//! The pure fold from `claude --output-format stream-json` frames to semantic
//! [`StreamEvent`]s, and the stdin encoders for a turn and an interrupt.
//!
//! Kept free of I/O so the whole protocol is unit-testable against literal JSON
//! lines — the shapes here were verified against a live `claude` CLI, and a
//! regression in them is silent otherwise.

use serde_json::{json, Value};

/// A semantic event folded out of one harness stream frame.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// The harness announced its own session id (`system`/`init`).
    Session {
        /// The CLI's opaque session id.
        session_id: String,
    },
    /// A chunk of the assistant's answer text.
    AssistantDelta {
        /// The text chunk.
        text: String,
    },
    /// A chunk of the assistant's reasoning.
    ReasoningDelta {
        /// The text chunk.
        text: String,
    },
    /// The assistant invoked a tool.
    Tool {
        /// A one-line summary, `name · input`.
        label: String,
    },
    /// The turn ended. **This — not process exit — is the completion signal.**
    ///
    /// `claude` emits exactly one `result` per top-level turn, on stdout,
    /// carrying the answer and an error flag. It is top-level-only by
    /// construction, which is what makes it immune to the `SubagentStop`
    /// truncation trap that a hook-based signal falls into.
    Result {
        /// The turn's final answer text.
        reply: String,
        /// Whether the harness flagged the turn as an error. An interrupted turn
        /// arrives here too, as `subtype: error_during_execution`.
        is_error: bool,
        /// The session id, restated on the result frame.
        session_id: Option<String>,
    },
}

/// How many characters of a tool's input to keep in its summary line.
const TOOL_INPUT_CLIP: usize = 200;

/// Fold one parsed stream frame into zero or more semantic events.
///
/// Returns a `Vec` because a single `assistant` frame may carry several content
/// blocks. Unknown frame types (user echoes, control responses) fold to nothing
/// rather than erroring — the stream is an open vocabulary.
pub fn map_stream_frame(frame: &Value) -> Vec<StreamEvent> {
    let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    match frame_type {
        "system" => {
            let is_init = frame.get("subtype").and_then(Value::as_str) == Some("init");
            match (is_init, frame.get("session_id").and_then(Value::as_str)) {
                (true, Some(session_id)) if !session_id.is_empty() => vec![StreamEvent::Session {
                    session_id: session_id.to_string(),
                }],
                _ => Vec::new(),
            }
        }
        "assistant" => fold_assistant(frame.get("message")),
        "result" => vec![StreamEvent::Result {
            reply: frame
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            is_error: frame
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            session_id: frame
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }],
        _ => Vec::new(),
    }
}

/// Fold an `assistant` frame's content blocks.
fn fold_assistant(message: Option<&Value>) -> Vec<StreamEvent> {
    let Some(blocks) = message
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(|block| {
            let text = |key: &str| {
                block
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
            };
            match block.get("type").and_then(Value::as_str)? {
                "text" => text("text").map(|text| StreamEvent::AssistantDelta { text }),
                "thinking" => text("thinking").map(|text| StreamEvent::ReasoningDelta { text }),
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    Some(StreamEvent::Tool {
                        label: format!("{name} · {}", summarize_input(block.get("input"))),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// Render a tool's input as one clipped line.
fn summarize_input(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let rendered = serde_json::to_string(input).unwrap_or_default();
    if rendered.chars().count() <= TOOL_INPUT_CLIP {
        return rendered;
    }
    let clipped: String = rendered.chars().take(TOOL_INPUT_CLIP).collect();
    format!("{clipped}…")
}

/// Encode one turn as a stdin line.
///
/// `claude --input-format stream-json` reads one JSON user message per line.
pub fn encode_user_message(text: &str) -> String {
    let frame = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{ "type": "text", "text": text }],
        },
    });
    format!("{frame}\n")
}

/// Encode an in-band interrupt as a stdin line.
///
/// This is a genuine per-turn abort, not a process kill: it tears down the
/// in-flight turn (including a blocking foreground tool) and makes the CLI emit
/// a terminating `result` frame **without exiting**, so an unbound session
/// survives its own interrupt and accepts the next turn.
pub fn encode_interrupt(request_id: &str) -> String {
    let frame = json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    });
    format!("{frame}\n")
}
