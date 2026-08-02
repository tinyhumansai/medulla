//! Kind-specific presentations for workflow step configuration.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

use super::syntax;
use super::types::AgentDefaults;
use super::{prompt, types::PromptTemplate};

/// Render a node according to the semantics of its kind.
pub(super) fn kind_lines(
    kind: &str,
    config: &Value,
    width: usize,
    agent_defaults: &AgentDefaults,
) -> Vec<Line<'static>> {
    match kind {
        "code" => code_lines(
            config
                .get("source")
                .or_else(|| config.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            config
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("javascript"),
            width,
        ),
        "agent" => agent_lines(config, agent_defaults),
        "condition" => labelled_value(
            "condition",
            config
                .get("expression")
                .or_else(|| config.get("condition"))
                .unwrap_or(&Value::Null),
        ),
        "http_request" => request_lines(config),
        "tool_call" if config.get("slug").and_then(Value::as_str) == Some("medulla:shell") => {
            let args = config.get("args").unwrap_or(&Value::Null);
            let source = args
                .get("script")
                .and_then(Value::as_str)
                .map(redact_source)
                .unwrap_or_default();
            code_lines(
                &source,
                args.get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("shell"),
                width,
            )
        }
        "tool_call" => tool_lines(config),
        "transform" | "set" | "merge" => labelled_value("mapping", config),
        "trigger" => labelled_value(
            "trigger",
            config.get("trigger_kind").unwrap_or(&Value::Null),
        ),
        _ => labelled_value("config", config),
    }
}

/// Render executable source with a language badge and line-number gutter.
fn code_lines(source: &str, language: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {language} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  executable source",
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])];
    if source.is_empty() {
        lines.push(Line::from(Span::styled(
            "No source configured.",
            Style::default().fg(Color::Yellow),
        )));
        return lines;
    }
    let count = source.lines().count();
    let gutter = count.to_string().len();
    for (index, source_line) in source.lines().enumerate() {
        let source_width = width.saturating_sub(gutter + 3).max(1);
        for (segment, highlighted) in
            syntax::wrap_spans(syntax::highlight_line(source_line, language), source_width)
                .into_iter()
                .enumerate()
        {
            let mut spans = vec![Span::styled(
                if segment == 0 {
                    format!("{:>gutter$} │ ", index + 1)
                } else {
                    format!("{:>gutter$} │ ", "")
                },
                Style::default().fg(Color::DarkGray),
            )];
            spans.extend(highlighted);
            lines.push(Line::from(spans));
        }
    }
    lines
}

/// Render a coding-agent step as a readable route followed by its task.
fn agent_lines(config: &Value, defaults: &AgentDefaults) -> Vec<Line<'static>> {
    let selected_worker = config
        .get("agent_ref")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|worker| !worker.is_empty());
    let (worker, route_note, missing) = match selected_worker {
        Some(worker) => (worker, "named workflow agent", false),
        None if !defaults.worker.is_empty() => {
            (defaults.worker.as_str(), "default workflow agent", false)
        }
        None => (
            "not configured",
            "set agent_ref or workflows.defaultWorker",
            true,
        ),
    };
    let effective = defaults.resolve(config);
    let worker_style = if missing {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("agent    ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(worker.to_string(), worker_style),
            Span::styled(
                format!("  · {route_note}"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(vec![
            Span::styled("harness  ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                effective.harness.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  · model {}  · {}", effective.model, effective.source),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(Span::styled(
            "A fresh, bounded harness session is started for this step.",
            Style::default().add_modifier(Modifier::DIM),
        )),
    ];
    if let Some(message) = &effective.unreadable {
        lines.push(Line::from(Span::styled(
            format!("harness   {message}"),
            Style::default().fg(Color::Red),
        )));
    }
    if config
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        lines.push(Line::from(Span::styled(
            "approval  required before dispatch",
            Style::default().fg(Color::Yellow),
        )));
    }
    let prompt = config
        .get("prompt")
        .or_else(|| config.get("instruction"))
        .or_else(|| config.get("task"))
        .unwrap_or(&Value::Null);
    lines.extend(task_lines(prompt));
    lines
}

/// Explain a literal task or a run-time binding in operator-facing language.
fn task_lines(prompt: &Value) -> Vec<Line<'static>> {
    let Some(text) = prompt.as_str() else {
        return labelled_value("task", prompt);
    };
    let dynamic = text.starts_with('=') || text.contains("{{");
    if !dynamic {
        return labelled_value("task", prompt);
    }
    if let Some(template) = prompt::decode_expression(text) {
        return prompt_template_lines(template);
    }

    let explanation = text
        .strip_prefix("=item.")
        .filter(|field| !field.is_empty())
        .map(|field| format!("Uses “{field}” from the previous step when this run reaches it."))
        .unwrap_or_else(|| "Built from workflow data when this run reaches the step.".to_string());
    vec![
        Line::from(vec![
            Span::styled(
                "dynamic task  ",
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::styled(explanation, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("  binding  ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(text.to_string(), Style::default().fg(Color::Magenta)),
        ]),
    ]
}

/// Render decoded prompt prose with compact names for its dynamic inputs.
fn prompt_template_lines(template: PromptTemplate) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "prompt template",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        template
            .text
            .lines()
            .map(|line| Line::from(Span::raw(format!("  {line}")))),
    );
    lines.push(Line::from(vec![
        Span::styled(
            "dynamic input  ",
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            template.inputs.join(", "),
            Style::default().fg(Color::Magenta),
        ),
    ]));
    lines
}

/// Render an HTTP request without exposing stored credential-shaped values.
fn request_lines(config: &Value) -> Vec<Line<'static>> {
    let method = config
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .map(safe_preview_url)
        .unwrap_or_else(|| "no URL".to_string());
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {method} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {url}")),
    ])];
    let rest = without_keys(config, &["method", "url"]);
    if !rest.is_empty() {
        lines.extend(labelled_value("request", &Value::Object(rest)));
    }
    lines
}

/// Strip URL components that commonly carry credentials before rendering.
fn safe_preview_url(raw: &str) -> String {
    let visible_end = raw.find(['?', '#']).unwrap_or(raw.len());
    let base = &raw[..visible_end];
    let Some(scheme_end) = base.find("://") else {
        return base.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = base[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(base.len());
    let authority = &base[authority_start..authority_end];
    let Some((_, host)) = authority.rsplit_once('@') else {
        return base.to_string();
    };
    format!(
        "{}{}{}",
        &base[..authority_start],
        host,
        &base[authority_end..]
    )
}

/// Render a non-shell tool invocation with its slug and arguments.
fn tool_lines(config: &Value) -> Vec<Line<'static>> {
    let slug = config
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("unknown tool");
    let mut lines = vec![Line::from(vec![
        Span::styled("tool  ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(slug.to_string(), Style::default().fg(Color::Magenta)),
    ])];
    lines.extend(labelled_value(
        "arguments",
        config.get("args").unwrap_or(&Value::Null),
    ));
    lines
}

/// Render a labelled scalar or pretty, recursively redacted JSON value.
pub(super) fn labelled_value(label: &str, value: &Value) -> Vec<Line<'static>> {
    let value = redact(value);
    let text = match &value {
        Value::String(text) => text.clone(),
        Value::Null => "not configured".to_string(),
        value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    };
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::from(Span::styled(format!("{label}  "), dim))];
    lines.extend(
        text.lines()
            .map(|line| Line::from(Span::raw(format!("  {line}")))),
    );
    lines
}

/// Scrub credentials embedded in executable source before syntax highlighting.
///
/// Shell is free-form text rather than structured JSON, so it needs both the
/// shared secret scanner and URL sanitization. Credential-shaped lines that the
/// scanner cannot safely edit are hidden in full.
fn redact_source(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let (redacted, _) = medulla::history_upload::redact_text(line);
            // Recheck what remains: one recognized secret must not mask a
            // second credential form the structured scanner cannot edit.
            let visible = if credential_shaped_source(&redacted) {
                "[credential-bearing source redacted]".to_string()
            } else {
                redacted
            };
            redact_source_urls(&visible)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Treat credential vocabulary as sensitive even when its value is unusually
/// short or uses a scheme the structured scanner does not recognize.
fn credential_shaped_source(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "authorization",
        "bearer ",
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "access_key",
        "private_key",
        "cookie",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Sanitize URL-shaped shell tokens without disturbing the surrounding source.
fn redact_source_urls(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = next_url_start(rest) {
        output.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url
            .find(|ch: char| {
                ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '}' | ';' | ',' | '|')
            })
            .unwrap_or(url.len());
        output.push_str(&safe_preview_url(&url[..end]));
        rest = &url[end..];
    }
    output.push_str(rest);
    output
}

/// Find a URI scheme without assuming lowercase HTTP spelling.
fn next_url_start(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut search_from = 0;
    while let Some(relative) = source[search_from..].find("://") {
        let marker = search_from + relative;
        let mut start = marker;
        while start > 0
            && bytes[start - 1].is_ascii()
            && (bytes[start - 1].is_ascii_alphanumeric()
                || matches!(bytes[start - 1], b'+' | b'-' | b'.'))
        {
            start -= 1;
        }
        let scheme = &source[start..marker];
        if scheme
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        {
            return Some(start);
        }
        search_from = marker + 3;
    }
    None
}

/// Copy an object except for keys already represented in its headline.
fn without_keys(value: &Value, excluded: &[&str]) -> Map<String, Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !excluded.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Recursively replace credential-shaped values before they reach the screen.
fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase().replace('-', "_");
                    let secret = [
                        "token",
                        "secret",
                        "password",
                        "api_key",
                        "apikey",
                        "authorization",
                        "cookie",
                    ]
                    .iter()
                    .any(|needle| lower.contains(needle));
                    (
                        key.clone(),
                        if secret {
                            Value::String("••••".to_string())
                        } else if let Value::String(text) = value {
                            if text.contains("://") {
                                Value::String(safe_preview_url(text))
                            } else {
                                value.clone()
                            }
                        } else {
                            redact(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        value => value.clone(),
    }
}
