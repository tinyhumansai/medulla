//! The Status line page's preview: three sample harness rows drawn through the
//! real rail renderer, inside a rail-width frame.
//!
//! Sampling rather than describing is deliberate. The rail row is thirty-six
//! columns shared between six fields, so what a placement costs the fields
//! beside it is not something prose can convey — only the row itself can.

use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use medulla::protocol::HarnessProvider;

use crate::ui::app::render::agents::RAIL_MAX_CONTENT;
use crate::ui::app::types::App;
use crate::worker::pty::{PtyState, SessionControl, SessionRow};

impl App {
    /// The preview: three sample harness rows inside a rail-width frame, each
    /// captioned with the condition it stands for.
    ///
    /// Three, not one, because "when selected" and "on alert" are answers about
    /// rows the operator is *not* looking at — a single always-selected, always-
    /// healthy sample would render those two choices unpreviewable, which is the
    /// one thing this page exists to prevent.
    pub(super) fn status_line_preview(&self, dim: Style) -> Vec<TLine<'static>> {
        let width = RAIL_MAX_CONTENT;
        let rule = |left: &str, right: &str| {
            TLine::from(Span::styled(
                format!("  {left}{}{right}", "─".repeat(width)),
                dim,
            ))
        };

        // One healthy row under the cursor, one healthy row that is not, and
        // one that needs attention: together they preview every visibility.
        let samples = [
            (sample_selected as fn() -> SessionRow, true, "selected"),
            (sample_orchestrator, false, "not selected"),
            (sample_alerting, false, "on alert"),
        ];
        let mut lines = vec![rule("┌", "┐")];
        for (index, (sample, active, caption)) in samples.iter().enumerate() {
            if index > 0 {
                lines.push(rule("├", "┤"));
            }
            let row = sample();
            for (offset, line) in self
                .own_session_lines(&row, *active, width, medulla::clock::now_millis())
                .into_iter()
                .enumerate()
            {
                let used: usize = line.spans.iter().map(|span| span.content.width()).sum();
                let mut spans = vec![Span::styled("  │", dim)];
                spans.extend(line.spans);
                spans.push(Span::styled(
                    format!("{}│", " ".repeat(width.saturating_sub(used))),
                    dim,
                ));
                // Caption the sample once, beside its first line.
                if offset == 0 {
                    spans.push(Span::styled(format!("  {caption}"), dim));
                }
                lines.push(TLine::from(spans));
            }
        }
        lines.push(rule("└", "┘"));
        lines
    }
}

/// A running, operator-held harness in a Git checkout — the common case, and the
/// one whose path is long enough to show what the layout costs it.
fn sample_selected() -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "preview".into(),
        label: "preview".into(),
        provider: HarnessProvider::Claude,
        preset: None,
        state: PtyState::Running,
        cwd: "/home/you/work/tinyhumans/medulla-public".into(),
        branch: Some("feat/status-line".into()),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: Some("Ship the status line".into()),
        started_at: 0,
        last_output_at: 0,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: crate::worker::pty::SessionOrigin::User,
        retained: false,
        name: None,
        attention: None,
        working: false,
    }
}

/// A finished, orchestrator-held harness outside a repository — the other half
/// of what the state, control, and branch rows can produce.
fn sample_orchestrator() -> SessionRow {
    SessionRow {
        provider: HarnessProvider::Codex,
        preset: None,
        state: PtyState::Exited { code: Some(0) },
        cwd: "/tmp/scratch".into(),
        branch: None,
        control: SessionControl::Orchestrator,
        ..sample_selected()
    }
}

/// A harness whose child died — what an "on alert" field is waiting for.
fn sample_alerting() -> SessionRow {
    SessionRow {
        state: PtyState::Failed,
        last_error: Some("session exited unexpectedly".into()),
        ..sample_selected()
    }
}
