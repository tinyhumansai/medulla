//! The hook document shape both Claude Code and Codex accept, and the inline
//! TOML encoder Codex's `-c` override needs.
//!
//! Claude Code 2.1.221 and Codex 0.146 read the same document: an object keyed
//! by event name, whose values are matcher groups, each holding a list of
//! command handlers. Building it once here is what makes "define a Medulla hook,
//! get it on every harness" a translation of the container rather than of the
//! hook itself.

use serde_json::{json, Map, Value};

use super::types::HookSpec;

/// Build the shared hook document for `hooks`, keyed by event name.
///
/// Hooks that name the same event and matcher are folded into one matcher group
/// so the harness runs them in config order, which is how both harnesses treat a
/// group's `hooks` array.
pub fn hook_document(hooks: &[&HookSpec]) -> Value {
    let mut events: Map<String, Value> = Map::new();
    for hook in hooks {
        let groups = events
            .entry(hook.event.as_str().to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };

        let handler = handler_value(hook);
        // Fold into an existing group with the same matcher; both harnesses run a
        // group's handlers in order, so appending preserves config order.
        let existing = groups.iter_mut().find(|group| {
            group.get("matcher").and_then(Value::as_str) == Some(hook.matcher.as_str())
        });
        match existing {
            Some(group) => {
                if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    list.push(handler);
                }
            }
            None => groups.push(json!({
                "matcher": hook.matcher,
                "hooks": [handler],
            })),
        }
    }
    Value::Object(events)
}

/// One `{"type":"command","command":…}` handler, carrying its timeout when set.
fn handler_value(hook: &HookSpec) -> Value {
    let mut handler = Map::new();
    handler.insert("type".into(), json!("command"));
    handler.insert("command".into(), json!(hook.command()));
    if let Some(timeout) = hook.timeout() {
        handler.insert("timeout".into(), json!(timeout));
    }
    Value::Object(handler)
}

/// Encode `value` as a single-line inline TOML expression.
///
/// Codex takes its config overrides as `-c key=<toml>`, and an override that
/// spans lines is not expressible on a command line — so the whole hook document
/// has to arrive as one inline table. Only the shapes [`hook_document`] produces
/// are reachable here; anything else encodes to its nearest TOML equivalent
/// rather than failing, since a malformed override would take the spawn down.
///
/// Null has no TOML spelling and is omitted by the table and array arms, so it
/// never reaches the scalar arm from a well-formed document.
pub fn to_inline_toml(value: &Value) -> String {
    match value {
        Value::Null => "\"\"".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => toml_string(text),
        Value::Array(items) => {
            let encoded: Vec<String> = items
                .iter()
                .filter(|item| !item.is_null())
                .map(to_inline_toml)
                .collect();
            format!("[{}]", encoded.join(","))
        }
        Value::Object(fields) => {
            let encoded: Vec<String> = fields
                .iter()
                .filter(|(_, field)| !field.is_null())
                .map(|(key, field)| format!("{}={}", toml_key(key), to_inline_toml(field)))
                .collect();
            format!("{{{}}}", encoded.join(","))
        }
    }
}

/// Quote a table key. Every key we emit is an event name or a fixed field name,
/// but quoting unconditionally keeps the encoder correct for any key.
fn toml_key(key: &str) -> String {
    toml_string(key)
}

/// Encode `text` as a TOML basic string.
///
/// Hook commands are arbitrary operator shell — quotes, backslashes and tabs are
/// ordinary in them — so escaping is what stops a command from breaking out of
/// the override and being reparsed as config.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // TOML basic strings reject raw control characters.
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
