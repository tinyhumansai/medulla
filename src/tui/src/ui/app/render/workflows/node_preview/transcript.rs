//! Rendering the harness transcript recorded for a finished step.
//!
//! The counterpart to [`super::live`], for a run that has already settled.
//! `live` shows the output a step is producing *right now*, which exists only
//! while a harness is attached; this shows what a step said when nobody was
//! watching, read back off the run record.
//!
//! Kept apart from [`super::run_detail`] because the shaping rules are
//! different in kind. Prompt and output are single values rendered with the
//! shared `labelled_value`; a transcript is a *sequence*, and the questions it
//! raises — which rows earn a colour, how many fit before the pane stops being
//! readable, how a long line is folded — have no counterpart there.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use medulla::harness_transcript::TranscriptEntry;

/// Rows drawn before the rest are summarized away.
///
/// The recorded transcript is already bounded (`MAX_ENTRIES`), but that bound
/// is sized for a file someone may read in full — far more than fits in a pane
/// beside a graph. An operator who wants the whole thing has
/// `workflow_run_get { steps: "full" }`; what belongs here is enough to see
/// the shape of the turn.
const MAX_ROWS: usize = 40;

/// Characters of one entry kept on its row.
///
/// One line each, deliberately: a transcript's value in this pane is the
/// *sequence* — twenty rows showing a loop of failing tool calls says more than
/// three rows showing their full output. The whole text is a `full` fetch away.
const MAX_ROW_CHARS: usize = 160;

/// Render a step's recorded transcript, or nothing when it has none.
///
/// Returning an empty vector for an empty transcript rather than a "no
/// transcript" row is deliberate: most nodes are not agent nodes, and a line
/// announcing the absence of something they never had would be on almost every
/// step in the pane.
pub(super) fn transcript_lines(transcript: &[TranscriptEntry]) -> Vec<Line<'static>> {
    if transcript.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::from(vec![
        Span::styled("said ", dim),
        Span::styled(
            format!(
                "{} entr{}",
                transcript.len(),
                if transcript.len() == 1 { "y" } else { "ies" }
            ),
            dim,
        ),
    ])];

    // The *last* rows, not the first. When a turn is longer than the pane, the
    // part worth seeing is how it ended — the error, the final message — and
    // the head is the part an operator can already predict from the prompt.
    let elided = transcript.len().saturating_sub(MAX_ROWS);
    if elided > 0 {
        lines.push(Line::from(Span::styled(
            format!("      … {elided} earlier entries not shown"),
            dim,
        )));
    }
    for entry in transcript.iter().skip(elided) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", short_kind(&entry.kind)), dim),
            Span::styled(clip(&entry.text), style_for(&entry.kind)),
        ]));
    }
    lines
}

/// The column label for an event kind.
///
/// Shortened because the column is fixed-width and the full wire names
/// (`agent_message`, `tool_result`) spend a quarter of the pane restating the
/// same two words. An unrecognised kind is shown verbatim — a build that meets
/// a kind added after it was written should say what it saw rather than
/// silently file it under "other".
fn short_kind(kind: &str) -> &str {
    match kind {
        "agent_message" => "said",
        "agent_thinking" => "thought",
        "user_prompt" => "asked",
        "tool_call" => "ran",
        "tool_result" => "failed",
        "error" => "error",
        "truncated" => "…",
        other => other,
    }
}

/// The colour a row's text carries.
///
/// Only the two kinds that mean something went wrong are coloured, and
/// thinking is dimmed as the aside it is. Everything else is left at the
/// terminal's default: a pane where every row is coloured is one where colour
/// has stopped carrying information.
fn style_for(kind: &str) -> Style {
    match kind {
        "tool_result" | "error" => Style::default().fg(Color::Red),
        "agent_thinking" => Style::default().add_modifier(Modifier::DIM),
        "truncated" => Style::default().fg(Color::DarkGray),
        "tool_call" => Style::default().fg(Color::Cyan),
        _ => Style::default(),
    }
}

/// Flatten `text` onto one row, eliding what does not fit.
///
/// Newlines become spaces rather than wrapping: a row per entry is what makes
/// the sequence readable, and one multi-line tool result would otherwise push
/// every entry after it out of the pane.
fn clip(text: &str) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    if flat.chars().count() <= MAX_ROW_CHARS {
        return flat;
    }
    let kept: String = flat.chars().take(MAX_ROW_CHARS - 1).collect();
    format!("{kept}…")
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
