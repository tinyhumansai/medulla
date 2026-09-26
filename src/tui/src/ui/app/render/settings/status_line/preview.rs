//! The Preview tab: three sample harness rows drawn through the real rail
//! renderer, inside a rail-width frame.
//!
//! Sampling rather than describing is deliberate. The rail row is thirty-six
//! columns shared between six fields, so what a placement costs the fields
//! beside it is not something prose can convey — only the row itself can.

use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use medulla::protocol::HarnessProvider;

use crate::ui::app::render::sessions::RAIL_MAX_CONTENT;
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
        // The caption rides in the rule above its sample rather than in a
        // column beside it. Beside, it set the pane's minimum width at the
        // frame plus the longest caption; in the rule the block is as wide as
        // the rail row and no wider, which is what lets it sit in half a pane.
        let rule = |left: &str, right: &str, caption: &str| {
            let dashes = width.saturating_sub(caption.len() + 2);
            let lead = dashes / 2;
            TLine::from(Span::styled(
                format!(
                    " {left}{} {caption} {}{right}",
                    "─".repeat(lead),
                    "─".repeat(dashes - lead)
                ),
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
        let mut lines = vec![TLine::from(Span::styled(
            format!("{width} columns, as on the rail"),
            dim,
        ))];
        for (index, (sample, active, caption)) in samples.iter().enumerate() {
            lines.push(if index == 0 {
                rule("┌", "┐", caption)
            } else {
                rule("├", "┤", caption)
            });
            let row = sample();
            for line in self.own_session_lines(&row, *active, width, SAMPLE_NOW_MS) {
                let used: usize = line.spans.iter().map(|span| span.content.width()).sum();
                let mut spans = vec![Span::styled(" │", dim)];
                spans.extend(line.spans);
                spans.push(Span::styled(
                    format!("{}│", " ".repeat(width.saturating_sub(used))),
                    dim,
                ));
                lines.push(TLine::from(spans));
            }
        }
        lines.push(TLine::from(Span::styled(
            format!(" └{}┘", "─".repeat(width)),
            dim,
        )));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "sample sessions, at a fixed moment",
            dim,
        )));
        lines
    }
}

/// The moment the samples are drawn at, measured against their zeroed
/// timestamps.
///
/// A wall clock here read the sample's `started_at: 0` as an age of fifty-odd
/// years and printed it into the alert row, which is both nonsense and a moving
/// target between redraws. Forty-five seconds is what a harness that has just
/// stopped answering actually looks like.
const SAMPLE_NOW_MS: i64 = 45_000;

/// A running, operator-held harness in a linked worktree — the common case, and
/// the one whose path is long enough to show what the layout costs it.
fn sample_selected() -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "preview".into(),
        label: "preview".into(),
        provider: HarnessProvider::Claude,
        preset: None,
        state: PtyState::Running,
        cwd: "/home/you/work/worktrees/status-line/medulla-public".into(),
        checkout: medulla::ui::checkout::Checkout {
            repo: Some("medulla-public".into()),
            worktree: Some("status-line".into()),
            branch: Some("feat/status-line".into()),
            head: Some("a1b2c3d".into()),
        },
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
        closed_by_request: false,
        name: None,
        attention: None,
        working: false,
    }
}

/// A finished, orchestrator-held harness outside a repository — the other half
/// of what the state, control, branch, and worktree rows can produce.
fn sample_orchestrator() -> SessionRow {
    SessionRow {
        provider: HarnessProvider::Codex,
        preset: None,
        state: PtyState::Exited { code: Some(0) },
        cwd: "/tmp/scratch".into(),
        checkout: medulla::ui::checkout::Checkout::default(),
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
