//! Placeholder for the Subconscious tab: the always-on layer under the surface.
//!
//! The orchestrator is being rebuilt as a subconscious tier — parallel, cheap,
//! and quiet by default — rather than a conversation an operator drives. Nothing
//! it does is wired to a surface yet, so this tab names the three things that
//! will land here instead of showing an empty pane: what it filters on the way
//! in, what it learns from the difference, and what it escalates for a human to
//! approve.
//!
//! A named placeholder rather than a hidden tab, because the three sections are
//! the design: an operator who can see where approvals will appear knows the
//! layer is not going to act behind their back.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::App;

impl App {
    /// Draw the coming-soon card naming the three subconscious surfaces.
    ///
    /// Left-aligned inside a centred card: the body is a list of headings with a
    /// line under each, and centring a list costs the leading edge that makes it
    /// scan as one.
    pub(super) fn draw_subconscious(&mut self, frame: &mut Frame, area: Rect) {
        let card = crate::ui::layout::centered_percent(area, 70, 60);
        self.note_pane(card);
        let heading = Style::default().add_modifier(Modifier::DIM);
        // Every line is kept short enough not to wrap inside the card. The card
        // is a fraction of the pane, so a blurb that wraps costs two rows and
        // pushes the footer out of the block entirely on a short terminal.
        let body = Text::from(vec![
            Line::from(Span::styled(
                "The always-on cheap layer. Parallel, and quiet by default.",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Compares outcome against expectation, and escalates the difference."),
            Line::from(""),
            Line::from(Span::styled("Intake", heading)),
            Line::from("Filtering. Below threshold produces nothing at all."),
            Line::from(""),
            Line::from(Span::styled("Learnings", heading)),
            Line::from("Prediction error is the signal. Solved work moves down."),
            Line::from(""),
            Line::from(Span::styled("Approvals", heading)),
            Line::from("Escalation on impasse lands here, for a human to approve."),
            Line::from(""),
            Line::from(Span::styled(
                "Nothing here is live yet.",
                Style::default().fg(self.theme.dim_border),
            )),
        ]);
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: true })
                .block(self.panel("Subconscious · Coming soon")),
            card,
        );
    }
}
