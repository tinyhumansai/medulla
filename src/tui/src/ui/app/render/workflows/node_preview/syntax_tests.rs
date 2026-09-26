//! Tests for compact workflow source syntax highlighting.

use ratatui::style::{Color, Modifier};

use super::syntax::highlight_line;

/// Join spans to prove highlighting never changes the executable source.
fn rendered(spans: &[ratatui::text::Span<'_>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

#[test]
fn python_colours_keywords_strings_numbers_and_comments() {
    let spans = highlight_line("def greet(name = \"Ada\", count = 2): # hello", "python");

    assert_eq!(
        rendered(&spans),
        "def greet(name = \"Ada\", count = 2): # hello"
    );
    assert!(spans.iter().any(|span| {
        span.content == "def"
            && span.style.fg == Some(Color::Cyan)
            && span.style.add_modifier.contains(Modifier::BOLD)
    }));
    assert!(spans
        .iter()
        .any(|span| span.content == "\"Ada\"" && span.style.fg == Some(Color::Green)));
    assert!(spans
        .iter()
        .any(|span| span.content == "2" && span.style.fg == Some(Color::Magenta)));
    assert!(spans
        .iter()
        .any(|span| span.content == "# hello" && span.style.fg == Some(Color::DarkGray)));
}

#[test]
fn shell_colours_commands_variables_strings_and_comments() {
    let spans = highlight_line("echo \"$TARGET\" # destination", "bash");

    assert_eq!(rendered(&spans), "echo \"$TARGET\" # destination");
    assert!(spans.iter().any(|span| {
        span.content == "echo"
            && span.style.fg == Some(Color::Yellow)
            && span.style.add_modifier.contains(Modifier::BOLD)
    }));
    assert!(spans
        .iter()
        .any(|span| span.content == "\"$TARGET\"" && span.style.fg == Some(Color::Green)));
    assert!(spans
        .iter()
        .any(|span| span.content == "# destination" && span.style.fg == Some(Color::DarkGray)));
}

#[test]
fn unknown_languages_preserve_a_plain_fallback() {
    let spans = highlight_line("launch <<opaque>>", "custom");

    assert_eq!(spans.len(), 1);
    assert_eq!(rendered(&spans), "launch <<opaque>>");
    assert_eq!(spans[0].style.fg, None);
}
