//! Human-readable decoding of workflow expressions that assemble agent prompts.
//!
//! Authored prompts commonly use jq concatenation: a large JSON string literal
//! plus one or more upstream values. Showing that program verbatim makes the
//! prose unreadable, so this module decodes literals and names dynamic inputs.

use super::types::PromptTemplate;

/// Decode a concatenated jq prompt expression into readable prose and inputs.
pub(super) fn decode_expression(expression: &str) -> Option<PromptTemplate> {
    let body = expression.strip_prefix('=')?.trim();
    let body = strip_outer_parentheses(body);
    let parts = split_concatenation(body)?;
    let mut text = String::new();
    let mut inputs = Vec::new();
    let mut found_literal = false;

    for part in parts {
        if let Ok(literal) = serde_json::from_str::<String>(part) {
            text.push_str(&literal);
            found_literal = true;
        } else {
            let input = describe_input(part);
            text.push_str(&format!("${{{}}}", variable_name(part)));
            inputs.push(input);
        }
    }

    (found_literal && !inputs.is_empty()).then_some(PromptTemplate { text, inputs })
}

/// Remove one balanced pair surrounding the complete expression.
fn strip_outer_parentheses(mut expression: &str) -> &str {
    loop {
        let trimmed = expression.trim();
        if !trimmed.starts_with('(') || !trimmed.ends_with(')') {
            return trimmed;
        }
        let mut depth = 0_usize;
        let mut quoted = false;
        let mut escaped = false;
        let mut encloses_all = true;
        for (offset, character) in trimmed.char_indices() {
            if quoted {
                if character == '"' && !escaped {
                    quoted = false;
                }
                escaped = character == '\\' && !escaped;
                if character != '\\' {
                    escaped = false;
                }
                continue;
            }
            match character {
                '"' => quoted = true,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && offset + 1 < trimmed.len() {
                        encloses_all = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if !encloses_all || depth != 0 {
            return trimmed;
        }
        expression = &trimmed[1..trimmed.len() - 1];
    }
}

/// Split top-level `+` operands without splitting strings or nested programs.
fn split_concatenation(expression: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;

    for (offset, character) in expression.char_indices() {
        if quoted {
            if character == '"' && !escaped {
                quoted = false;
            }
            escaped = character == '\\' && !escaped;
            if character != '\\' {
                escaped = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '+' if depth == 0 => {
                parts.push(expression[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    parts.push(expression[start..].trim());

    (parts.len() > 1 && parts.iter().all(|part| !part.is_empty())).then_some(parts)
}

/// Turn a jq path into a compact interpolation-style variable name.
fn variable_name(expression: &str) -> String {
    let path = expression.trim().trim_start_matches('.');
    let fields = path.split('.').collect::<Vec<_>>();
    if let Some(node_index) = fields.iter().position(|field| *field == "nodes") {
        if let Some(node) = fields.get(node_index + 1) {
            let leaf = fields
                .iter()
                .position(|field| *field == "output")
                .and_then(|index| fields.get(index + 1))
                .or_else(|| fields.last())
                .copied()
                .unwrap_or("output");
            return format!("{node}.{leaf}");
        }
    }
    path.to_string()
}

/// Turn a jq data path into the short label an operator cares about.
fn describe_input(expression: &str) -> String {
    let path = expression.trim().trim_start_matches('.');
    let fields = path.split('.').collect::<Vec<_>>();
    if let Some(node_index) = fields.iter().position(|field| *field == "nodes") {
        if let Some(node) = fields.get(node_index + 1) {
            let leaf = fields
                .iter()
                .position(|field| *field == "output")
                .and_then(|index| fields.get(index + 1))
                .or_else(|| fields.last())
                .copied()
                .unwrap_or("output");
            return format!("{node} → {leaf}");
        }
    }
    if fields.first() == Some(&"item") {
        return format!(
            "previous step → {}",
            fields.last().copied().unwrap_or("output")
        );
    }
    path.to_string()
}
