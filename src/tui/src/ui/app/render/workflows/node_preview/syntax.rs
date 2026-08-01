//! Lightweight syntax highlighting for executable workflow source.
//!
//! The preview deliberately avoids a parser dependency: it colours the tokens
//! people scan for most often while preserving every source character exactly.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;

/// Syntax families whose common tokens can be highlighted reliably.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Shell,
    Python,
    JavaScript,
    Rust,
    Data,
    Unknown,
}

/// Colour one source line while preserving its exact text and spacing.
pub(super) fn highlight_line(line: &str, language: &str) -> Vec<Span<'static>> {
    let language = Language::from_name(language);
    if language == Language::Unknown || line.is_empty() {
        return vec![Span::raw(line.to_string())];
    }

    let mut spans = Vec::new();
    let mut cursor = 0;
    let mut shell_command_pending = language == Language::Shell;
    while cursor < line.len() {
        let rest = &line[cursor..];
        if is_comment_start(rest, language) {
            spans.push(Span::styled(rest.to_string(), comment_style()));
            break;
        }

        let first = rest.chars().next().expect("cursor remains in the line");
        if first.is_whitespace() {
            let length = take_while(rest, char::is_whitespace);
            spans.push(Span::raw(rest[..length].to_string()));
            cursor += length;
            continue;
        }
        if matches!(first, '\'' | '"' | '`') {
            let length = quoted_length(rest, first);
            spans.push(Span::styled(
                rest[..length].to_string(),
                Style::default().fg(Color::Green),
            ));
            cursor += length;
            shell_command_pending = false;
            continue;
        }
        if first.is_ascii_digit() {
            let length = take_while(rest, |character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_')
            });
            spans.push(Span::styled(
                rest[..length].to_string(),
                Style::default().fg(Color::Magenta),
            ));
            cursor += length;
            shell_command_pending = false;
            continue;
        }
        if identifier_start(first) {
            let length = take_while(rest, identifier_continue);
            let token = &rest[..length];
            let style = token_style(token, language, shell_command_pending);
            spans.push(Span::styled(token.to_string(), style));
            cursor += length;
            if language == Language::Shell && token != "sudo" && token != "env" {
                shell_command_pending = false;
            }
            continue;
        }

        let length = first.len_utf8();
        spans.push(Span::raw(rest[..length].to_string()));
        cursor += length;
    }
    spans
}

/// Wrap highlighted spans without losing their styles or source characters.
pub(super) fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines = vec![Vec::new()];
    let mut used = 0;

    for span in spans {
        let mut part = String::new();
        for character in span.content.chars() {
            let character_width = character.width().unwrap_or(0);
            if used > 0 && used + character_width > width {
                push_part(
                    lines.last_mut().expect("one line exists"),
                    &mut part,
                    span.style,
                );
                lines.push(Vec::new());
                used = 0;
            }
            part.push(character);
            used += character_width;
        }
        push_part(
            lines.last_mut().expect("one line exists"),
            &mut part,
            span.style,
        );
    }
    lines
}

/// Append an owned span when a wrapped token has accumulated text.
fn push_part(line: &mut Vec<Span<'static>>, part: &mut String, style: Style) {
    if !part.is_empty() {
        line.push(Span::styled(std::mem::take(part), style));
    }
}

impl Language {
    /// Resolve common workflow language labels to their syntax family.
    fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "sh" | "shell" | "bash" | "zsh" => Self::Shell,
            "py" | "python" | "python3" => Self::Python,
            "js" | "jsx" | "javascript" | "ts" | "tsx" => Self::JavaScript,
            "rs" | "rust" => Self::Rust,
            "json" | "jsonc" | "toml" | "yaml" | "yml" => Self::Data,
            _ => Self::Unknown,
        }
    }
}

/// Style identifiers according to their semantic role.
fn token_style(token: &str, language: Language, shell_command_pending: bool) -> Style {
    if is_keyword(token, language) {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if language == Language::Shell && token.starts_with('$') {
        Style::default().fg(Color::Cyan)
    } else if language == Language::Shell && shell_command_pending {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if matches!(token, "true" | "false" | "True" | "False" | "null" | "None") {
        Style::default().fg(Color::Magenta)
    } else {
        Style::default()
    }
}

/// Return whether the remaining text begins a line or trailing comment.
fn is_comment_start(rest: &str, language: Language) -> bool {
    match language {
        Language::Shell | Language::Python => rest.starts_with('#'),
        Language::JavaScript | Language::Rust => rest.starts_with("//"),
        Language::Data => rest.starts_with('#') || rest.starts_with("//"),
        Language::Unknown => false,
    }
}

/// Find the byte length of a quoted token, including an unterminated suffix.
fn quoted_length(text: &str, quote: char) -> usize {
    let mut escaped = false;
    for (offset, character) in text.char_indices().skip(1) {
        if character == quote && !escaped {
            return offset + character.len_utf8();
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    text.len()
}

/// Find the byte length of the leading characters matching a predicate.
fn take_while(text: &str, predicate: impl Fn(char) -> bool) -> usize {
    text.char_indices()
        .find_map(|(offset, character)| (!predicate(character)).then_some(offset))
        .unwrap_or(text.len())
}

/// Return whether a character can begin an identifier or shell variable.
fn identifier_start(character: char) -> bool {
    character.is_alphabetic() || matches!(character, '_' | '$')
}

/// Return whether a character can continue an identifier.
fn identifier_continue(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$' | '-')
}

/// Recognise the small, stable keyword set useful in a compact viewer.
fn is_keyword(token: &str, language: Language) -> bool {
    match language {
        Language::Shell => matches!(
            token,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "while"
                | "in"
                | "do"
                | "done"
                | "case"
                | "esac"
                | "function"
        ),
        Language::Python => matches!(
            token,
            "and"
                | "as"
                | "async"
                | "await"
                | "class"
                | "def"
                | "elif"
                | "else"
                | "except"
                | "for"
                | "from"
                | "if"
                | "import"
                | "in"
                | "lambda"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        ),
        Language::JavaScript => matches!(
            token,
            "async"
                | "await"
                | "class"
                | "const"
                | "else"
                | "export"
                | "extends"
                | "for"
                | "from"
                | "function"
                | "if"
                | "import"
                | "let"
                | "new"
                | "return"
                | "switch"
                | "throw"
                | "try"
                | "var"
                | "while"
        ),
        Language::Rust => matches!(
            token,
            "as" | "async"
                | "await"
                | "const"
                | "crate"
                | "else"
                | "enum"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "let"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "return"
                | "self"
                | "struct"
                | "trait"
                | "type"
                | "use"
                | "where"
                | "while"
        ),
        Language::Data | Language::Unknown => false,
    }
}

/// Keep comments legible but visually subordinate to executable tokens.
fn comment_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC)
}
