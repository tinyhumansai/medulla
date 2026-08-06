//! Human-readable decoding of workflow expressions that assemble agent prompts.
//!
//! Authored prompts commonly use jq concatenation: a large JSON string literal
//! plus one or more upstream values. Showing that program verbatim makes the
//! prose unreadable, so this module decodes literals and names dynamic inputs.
//!
//! An operand is rarely a bare path. Real workflows wrap them —
//! `(.nodes.attempt.iteration | tostring)`, `(.a.url // ("#" + .a.number))`,
//! `(if .inputs.include_tests then "…" else "…" end)` — and naming those by
//! their source text put the jq program back in the prose the decoding exists
//! to remove. [`operand`] peels the wrappers off first, so what a reader sees
//! is the value being interpolated rather than the expression that computes it.

use super::types::PromptTemplate;

/// What one dynamic operand of a concatenated prompt turns out to be.
#[derive(Debug, PartialEq, Eq)]
enum Operand {
    /// A data path, with its wrappers and leading dot removed.
    Path(String),
    /// A conditional choosing between texts, named by the path it tests.
    ///
    /// `None` when the condition is not a plain path — the choice is still
    /// worth naming as a choice, which is what an operator reading the prose
    /// needs to know is happening there.
    Choice(Option<String>),
    /// A program this module cannot reduce further, kept verbatim so nothing
    /// the author wrote is hidden from the one line that reports it.
    Program(String),
}

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
            let operand = operand(part);
            text.push_str(&format!("${{{}}}", variable_name(&operand)));
            inputs.push(describe_input(&operand));
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

/// Classify one dynamic operand, peeling off the wrappers authors write.
///
/// Order matters: parentheses come off first, then a conditional is recognized
/// before the pipeline split, because `if … then … else … end` legitimately
/// contains `|` and `//` inside its arms and splitting there would name the
/// operand after a fragment of one branch.
fn operand(part: &str) -> Operand {
    let body = strip_outer_parentheses(part.trim());
    if let Some(condition) = conditional_test(body) {
        return Operand::Choice(as_path(strip_outer_parentheses(condition)));
    }
    // `a | tostring` computes a *rendering* of `a`, and `a // b` a fallback for
    // it. Both are about the same value, so the left-most operand is the one
    // worth naming; the rest is machinery.
    let head = split_first(body, &["//", "|"]);
    // A head that is itself wrapped — `(.item.findings | tostring) // "none"`
    // splits to `(.item.findings | tostring)` — is not a path, so classifying
    // it directly would give up and print the jq source. Peel it the same way
    // instead. This recurses only when the split actually cut something, so it
    // terminates on a body with no top-level separator.
    if head != body {
        return match operand(head) {
            // Nothing legible came out of the head after all, so name the whole
            // expression rather than a fragment of it.
            Operand::Program(_) => Operand::Program(body.to_string()),
            classified => classified,
        };
    }
    match as_path(body) {
        Some(path) => Operand::Path(path),
        None => Operand::Program(body.to_string()),
    }
}

/// The condition of a top-level `if … then …`, when the operand is one.
fn conditional_test(body: &str) -> Option<&str> {
    let rest = body.strip_prefix("if")?;
    if !rest.starts_with(char::is_whitespace) && !rest.starts_with('(') {
        return None;
    }
    // As a *keyword*, not a substring: the condition is usually a path, and a
    // field with `then` inside it — `.inputs.authenticated` — would otherwise
    // cut the condition mid-identifier and name the operand `inputs.au`.
    let end = find_top_level_keyword(rest, "then")?;
    Some(rest[..end].trim())
}

/// The text before the first top-level occurrence of any of `separators`.
fn split_first<'a>(body: &'a str, separators: &[&str]) -> &'a str {
    separators
        .iter()
        .filter_map(|separator| find_top_level(body, separator))
        .min()
        .map(|end| body[..end].trim())
        .unwrap_or(body)
}

/// Where `needle` first appears outside quotes and brackets, if it does.
fn find_top_level(body: &str, needle: &str) -> Option<usize> {
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, character) in body.char_indices() {
        if quoted {
            if character == '"' && !escaped {
                quoted = false;
            }
            escaped = character == '\\' && !escaped;
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && !quoted && body[offset..].starts_with(needle) {
            return Some(offset);
        }
    }
    None
}

/// The dotted path an expression names, when it is nothing but a path.
///
/// Anything with an operator, a call, or a literal in it is not a path — and
/// answering "some sort of path" for those is what put jq programs back into
/// the prose.
fn as_path(expression: &str) -> Option<String> {
    let path = expression.trim().strip_prefix('.')?;
    if path.is_empty()
        || !path
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some(path.to_string())
}

/// The compact interpolation-style name an operand is written as in the prose.
fn variable_name(operand: &Operand) -> String {
    match operand {
        Operand::Path(path) => {
            let fields = path.split('.').collect::<Vec<_>>();
            if let Some((node, leaf)) = node_and_leaf(&fields) {
                return format!("{node}.{leaf}");
            }
            // A step's own input is addressed as `.item.…`, which is a fact
            // about how the engine passes it rather than something an operator
            // named — so the leaf alone reads better in the middle of a
            // sentence.
            if fields.first() == Some(&"item") {
                return fields.last().copied().unwrap_or("item").to_string();
            }
            path.clone()
        }
        Operand::Choice(Some(path)) => format!(
            "if {}",
            path.split('.').next_back().unwrap_or(path.as_str())
        ),
        Operand::Choice(None) => "if …".to_string(),
        Operand::Program(_) => "value".to_string(),
    }
}

/// The short label the `dynamic input` line reports an operand under.
fn describe_input(operand: &Operand) -> String {
    match operand {
        Operand::Path(path) => {
            let fields = path.split('.').collect::<Vec<_>>();
            if let Some((node, leaf)) = node_and_leaf(&fields) {
                return format!("{node} → {leaf}");
            }
            if fields.first() == Some(&"item") {
                return format!(
                    "previous step → {}",
                    fields.last().copied().unwrap_or("output")
                );
            }
            if fields.first() == Some(&"inputs") {
                return format!(
                    "workflow input → {}",
                    fields.last().copied().unwrap_or("input")
                );
            }
            path.clone()
        }
        Operand::Choice(Some(path)) => format!("{path} → one of two texts"),
        Operand::Choice(None) => "a condition → one of two texts".to_string(),
        // Reported verbatim: this is the one operand nothing above could
        // explain, and hiding it would leave an operator with a `${value}` in
        // the prose and no way to find out what fills it.
        Operand::Program(source) => source.clone(),
    }
}

/// The `(node, leaf)` a `.nodes.<id>.…` path addresses.
fn node_and_leaf<'a>(fields: &[&'a str]) -> Option<(&'a str, &'a str)> {
    let node_index = fields.iter().position(|field| *field == "nodes")?;
    let node = fields.get(node_index + 1)?;
    let leaf = fields
        .iter()
        .position(|field| *field == "output")
        .and_then(|index| fields.get(index + 1))
        .or_else(|| fields.last())
        .copied()
        .unwrap_or("output");
    Some((node, leaf))
}
