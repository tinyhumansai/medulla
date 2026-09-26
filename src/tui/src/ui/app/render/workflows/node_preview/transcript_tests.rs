//! Unit tests for transcript rendering in the run view.

use super::*;

/// One entry of `kind` saying `text`.
fn entry(kind: &str, text: &str) -> TranscriptEntry {
    TranscriptEntry {
        at_ms: 1_700_000_000_000,
        kind: kind.to_string(),
        text: text.to_string(),
    }
}

/// The flat text of a rendered line, for assertions.
fn flat(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Most steps are not agent nodes. A row announcing the absence of a
/// transcript would appear on nearly every step in the pane.
#[test]
fn a_step_with_no_transcript_renders_nothing() {
    assert!(transcript_lines(&[]).is_empty());
}

/// A header naming the size, then one row per entry.
#[test]
fn each_entry_gets_its_own_row_under_a_count() {
    let lines = transcript_lines(&[
        entry("agent_message", "on it"),
        entry("tool_call", "Bash(npm test)"),
    ]);

    assert_eq!(lines.len(), 3);
    assert!(flat(&lines[0]).contains("2 entries"), "{}", flat(&lines[0]));
    assert!(flat(&lines[1]).contains("said"));
    assert!(flat(&lines[1]).contains("on it"));
    assert!(flat(&lines[2]).contains("Bash(npm test)"));
}

/// The singular is spelled correctly — this row is read on every agent step.
#[test]
fn one_entry_is_not_reported_as_entries() {
    let lines = transcript_lines(&[entry("agent_message", "done")]);
    assert!(flat(&lines[0]).contains("1 entry"), "{}", flat(&lines[0]));
}

/// A turn longer than the pane keeps its *tail*: the error and the final
/// message are what an operator opened the run to find.
#[test]
fn an_overlong_transcript_keeps_its_last_rows_and_says_what_it_dropped() {
    let entries: Vec<_> = (0..MAX_ROWS + 5)
        .map(|index| entry("agent_message", &format!("line {index}")))
        .collect();

    let lines = transcript_lines(&entries);
    let rendered: Vec<String> = lines.iter().map(flat).collect();

    assert!(
        rendered[1].contains("5 earlier entries not shown"),
        "{}",
        rendered[1]
    );
    assert!(
        rendered.iter().any(|line| line.contains("line 44")),
        "the last entry must survive"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("line 0 ")),
        "the head is the part that is dropped"
    );
}

/// A multi-line tool result folds onto its own row instead of pushing every
/// later entry out of the pane.
#[test]
fn a_multiline_entry_is_flattened_onto_one_row() {
    let lines = transcript_lines(&[entry("tool_result", "failed\nline two\nline three")]);

    assert_eq!(lines.len(), 2);
    assert!(flat(&lines[1]).contains("failed line two line three"));
}

/// A very long entry is clipped rather than wrapped, and the clip is marked.
#[test]
fn a_long_entry_is_clipped_and_marked() {
    let lines = transcript_lines(&[entry("agent_message", &"x".repeat(500))]);

    let row = flat(&lines[1]);
    assert!(row.ends_with('…'), "the clip must be visible");
    assert!(row.chars().count() < 500);
}

/// A kind this build does not know is shown verbatim rather than filed under a
/// generic label — the wire vocabulary is additive, and guessing hides that.
#[test]
fn an_unfamiliar_kind_is_labelled_verbatim() {
    let lines = transcript_lines(&[entry("some_new_kind", "hello")]);
    assert!(flat(&lines[1]).contains("some_new_kind"));
}

/// Only failure carries colour. A pane where every row is coloured is one
/// where colour has stopped saying anything.
#[test]
fn only_failures_are_coloured_red() {
    assert_eq!(style_for("error").fg, Some(Color::Red));
    assert_eq!(style_for("tool_result").fg, Some(Color::Red));
    assert_eq!(style_for("agent_message").fg, None);
}
