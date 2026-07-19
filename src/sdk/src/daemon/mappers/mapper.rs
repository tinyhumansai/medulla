//! The stateful per-stream fold: [`HarnessLineMapper`]'s constructor, usage
//! accessor, and the per-line dispatch to the provider mappers, including the
//! codex duplicate-message dedupe and the token-usage scan.

use serde_json::Value;

use crate::tinyplace::TokenUsage;

use super::claude::claude_events_from_line;
use super::codex::codex_events_from_line;
use super::opencode::opencode_events_from_line;
use super::types::{HarnessLineMapper, HarnessSemanticEvent, Provider};
use super::usage::scan_usage;

/// Codex records each assistant message twice within this window; drop the repeat.
const CODEX_DUPLICATE_WINDOW_MS: i64 = 2000;

impl HarnessLineMapper {
    /// Build a mapper for `provider` (`"claude" | "codex" | "opencode"`).
    /// Unknown providers yield a mapper that emits nothing.
    pub fn new(provider: &str) -> Self {
        let provider = match provider {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => Provider::Opencode,
        };
        HarnessLineMapper {
            provider,
            last_text: None,
            last_at_ms: i64::MIN,
            usage: None,
        }
    }

    /// The most recent token usage seen on the stream, if any. Providers report
    /// cumulative counts (claude on the result record, codex via token_count
    /// events), so latest-wins is the correct fold.
    pub fn usage(&self) -> Option<TokenUsage> {
        self.usage
    }

    /// Map one raw JSONL line into zero or more semantic events.
    pub fn map_line(&mut self, raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
        // Token accounting rides on assorted records per provider; scan any
        // line that plausibly carries counts and keep the latest.
        if raw.contains("okens") {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                if let Some(usage) = scan_usage(&value, 0) {
                    self.usage = Some(usage);
                }
            }
        }
        let events = match self.provider {
            Provider::Claude => claude_events_from_line(raw, line),
            Provider::Codex => codex_events_from_line(raw, line),
            Provider::Opencode => opencode_events_from_line(raw, line),
        };
        if self.provider != Provider::Codex {
            return events;
        }
        // Codex dedupe: an agent_message whose text equals the previous one
        // within the window is the same message re-recorded, not a new turn.
        events
            .into_iter()
            .filter(|semantic| {
                if semantic.event.kind != "agent_message" {
                    return true;
                }
                let text = semantic
                    .event
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let at_ms = semantic.timestamp_ms;
                let duplicate = self.last_text.as_deref() == Some(text.as_str())
                    && (at_ms - self.last_at_ms).abs() <= CODEX_DUPLICATE_WINDOW_MS;
                self.last_text = Some(text);
                self.last_at_ms = at_ms;
                !duplicate
            })
            .collect()
    }
}
