//! Token-usage extraction: a depth-bounded scan that finds the input/output
//! token counts wherever a provider nests them on its records.

use serde_json::Value;

use crate::tinyplace_support::TokenUsage;

/// Depth-bounded scan for a token-usage object: any JSON object carrying both
/// input and output token counts, wherever the provider nests it (claude
/// `result.usage` and codex `token_count` payloads use `input_tokens`/
/// `output_tokens`; opencode nests a `tokens: { input, output, … }` object on
/// its step/message parts).
pub(super) fn scan_usage(value: &Value, depth: usize) -> Option<TokenUsage> {
    if depth > 4 {
        return None;
    }
    if let Some(obj) = value.as_object() {
        let num = |keys: [&str; 2]| {
            keys.iter()
                .find_map(|k| obj.get(*k))
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        };
        if let (Some(input), Some(output)) = (
            num(["input_tokens", "inputTokens"]),
            num(["output_tokens", "outputTokens"]),
        ) {
            return Some(TokenUsage {
                input_tokens: input,
                output_tokens: output,
            });
        }
        // opencode reports a nested `tokens: { input, output, reasoning, cache }`
        // object rather than the *_tokens naming the other harnesses use.
        if let Some(tokens) = obj.get("tokens").and_then(|v| v.as_object()) {
            let tnum = |key: &str| {
                tokens
                    .get(key)
                    .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            };
            if let (Some(input), Some(output)) = (tnum("input"), tnum("output")) {
                return Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                });
            }
        }
        return obj.values().find_map(|v| scan_usage(v, depth + 1));
    }
    None
}
