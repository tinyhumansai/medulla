//! Rendering the harness output a node is producing right now.
//!
//! The rest of this pane describes a step: what it is configured to do, and
//! what a finished run recorded for it. This is the third thing an operator
//! wants from the same box while a run is in flight — what the coding agent
//! under the step is *doing* — and it is drawn in the harness's own vocabulary
//! (tool calls, thinking, status) rather than as anonymous log lines, because
//! Medulla already classifies those frames for the copilot transcript and two
//! renderings of one stream would drift.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use medulla::ui::workflows::{classify_progress, Progress};

/// The most frames drawn, newest last.
///
/// The pane scrolls, but a live view is read from its tail: an agent step can
/// emit hundreds of frames, and paging back through all of them to reach the
/// current one is not what a "what is happening now" box is for.
const VISIBLE_FRAMES: usize = 40;

/// Draw the tail of one node's live harness output.
///
/// `running` distinguishes a step still working from one whose frames are the
/// last thing it said before the run ended — the same lines mean different
/// things, and a spinner over a finished run is a lie.
pub(super) fn live_lines(frames: &[String], running: bool) -> Vec<Line<'static>> {
    if frames.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            if running { " live " } else { " last " },
            Style::default()
                .fg(Color::Black)
                .bg(if running {
                    Color::Green
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if running {
                "  what this step's harness is doing"
            } else {
                "  what this step's harness was doing when the run ended"
            },
            dim,
        ),
    ])];

    let skipped = frames.len().saturating_sub(VISIBLE_FRAMES);
    if skipped > 0 {
        lines.push(Line::from(Span::styled(
            format!("  … {skipped} earlier frames"),
            dim,
        )));
    }
    for frame in frames.iter().skip(skipped) {
        lines.push(frame_line(frame));
    }
    lines
}

/// Strip control characters from a frame's text before it reaches a span.
///
/// Every payload here is a coding agent's own stdout, relayed verbatim — and
/// for a run started over MCP, relayed from another process entirely. Ratatui
/// does not neutralize what it is handed: an ESC or OSC sequence in a tool
/// argument or a thinking fragment reaches the operator's terminal and is
/// interpreted there, so a file a harness merely *read* could move the cursor or
/// rewrite the window title. Trimming stays here rather than at the call sites
/// because a sequence can hide behind leading whitespace.
fn inline_text(value: &str) -> String {
    value.trim().chars().filter(|c| !c.is_control()).collect()
}

/// One progress frame, marked up by what kind of frame it is.
fn frame_line(frame: &str) -> Line<'static> {
    match classify_progress(frame) {
        Progress::Tool { text, .. } => Line::from(vec![
            Span::styled("  ⏺ ", Style::default().fg(Color::Cyan)),
            Span::raw(inline_text(&text)),
        ]),
        Progress::ToolResult { failed, detail, .. } => Line::from(vec![
            Span::styled(
                if failed { "    ✗ " } else { "    ✓ " },
                Style::default().fg(if failed { Color::Red } else { Color::Green }),
            ),
            Span::styled(
                inline_text(&detail),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        Progress::Thinking(fragment) => Line::from(vec![
            Span::styled("  · ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                inline_text(&fragment),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]),
        Progress::Status(text) => Line::from(vec![
            Span::styled("  · ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                inline_text(&text),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
    }
}
