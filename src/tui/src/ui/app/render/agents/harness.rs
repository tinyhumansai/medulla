//! The selected agent's *actual* harness interface, drawn into the pane.
//!
//! This is what replaced the reconstructed transcript for locally hosted work.
//! The harness runs on a pseudo-terminal in its own interactive mode, an
//! emulator parses what it paints, and this module blits that grid into the
//! pane — so the operator is looking at Claude Code or Codex itself, not at our
//! rendering of the JSON it would have emitted had we started it headless.
//!
//! Resolution is deliberately conservative. A screen is shown only when exactly
//! one live session can be named for what the cursor is on; a lane whose tasks
//! run in two sessions shows the transcript instead, because picking one of them
//! would be showing work the operator did not point at.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::harness_pane::FOCUS_CHORD_LABEL;
use crate::worker::pty::ATTENTION_GLYPH;
use crate::worker::screen::screen_lines;

use super::super::super::types::App;
use super::types::Selection;

impl App {
    /// The live local harness session for whatever the cursor is on.
    ///
    /// A selected *task* resolves exactly: the host's runtime records which
    /// session serves which task, so there is nothing to guess. A selected
    /// *lane* resolves only when exactly one of its tasks has a live session —
    /// an agent usually has one harness open, and showing it saves the operator
    /// a keystroke, but two would make the choice arbitrary.
    ///
    /// `None` when this device does not host, when the work settled (the
    /// runtime drops the record then, so the pane stops claiming a screen for
    /// work that is over), or when the answer would be a guess.
    pub(super) fn local_session(&self, selection: &Selection) -> Option<String> {
        let harnesses = self.local_sessions.as_ref()?;
        // An operator-started harness row *is* a session — it names one
        // directly rather than through a task, which is the whole reason it
        // needs its own rail group: nothing ever dispatched into it, so the
        // runtime has no `(sender, task id)` record to resolve.
        if let Some(id) = selection
            .rows
            .get(selection.active)
            .and_then(|row| row.session_id())
        {
            return Some(id.to_string());
        }
        if let Some(task) = &selection.task {
            return harnesses.session_for_task(&task.task_id);
        }
        let lane = selection.lane()?;
        let mut sessions = lane
            .tasks
            .iter()
            .filter_map(|task| harnesses.session_for_task(&task.task_id));
        let only = sessions.next()?;
        sessions.next().is_none().then_some(only)
    }

    /// Draw the harness's own screen into `area`.
    ///
    /// Resizes the child to the pane first: the harness reflows to the PTY size,
    /// so an emulator of a different size would render a torn screen. When the
    /// pane is attached the harness's cursor is drawn too — an operator typing
    /// into a terminal with no cursor cannot tell where their text is going.
    pub(super) fn draw_local_harness(&mut self, f: &mut Frame, area: Rect, session_id: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            return;
        };
        let attached = self.harness_focus.is_attached_to(session_id);
        let block = self.panel(self.harness_title(session_id, attached));
        let inner = block.inner(area);
        // A border-styled block is the focus indicator: attached panes get a
        // bold border so the operator can see where their keys are going from
        // across the room, not only by reading the title.
        let block = if attached {
            block.border_style(Style::default().add_modifier(Modifier::BOLD))
        } else {
            block
        };
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Recorded before the paint so a wheel event landing between frames
        // still has somewhere to go.
        self.hit_session = Some((inner, session_id.to_string()));
        harnesses.fit(session_id, inner.width, inner.height);
        let Some(snapshot) = harnesses.screen(session_id) else {
            return;
        };
        // Clipped, never rewrapped. The grid is a framebuffer: reflowing it to
        // a narrower pane would not be the screen the harness is showing.
        f.render_widget(Paragraph::new(Text::from(screen_lines(&snapshot))), inner);

        // Only the attached pane places the cursor. Two panes both setting it
        // would leave whichever drew last owning it, which is how a cursor ends
        // up blinking in a pane nobody is typing into.
        if attached && !snapshot.hide_cursor {
            let (row, col) = snapshot.cursor;
            // The emulator's cursor can sit one past the last column on a full
            // line; clamping keeps it inside the pane rather than in the border.
            let x = inner.x + col.min(inner.width.saturating_sub(1));
            let y = inner.y + row.min(inner.height.saturating_sub(1));
            f.set_cursor_position((x, y));
        }
    }

    /// The pane title for a harness screen: what it is, and how to get in or
    /// out of it.
    ///
    /// The chord is in the title rather than in a help screen because it is the
    /// one thing an operator needs at exactly the moment they are looking at
    /// this pane — and because an embedded terminal with no visible way out is
    /// the reason people avoid them.
    fn harness_title(&self, session_id: &str, attached: bool) -> String {
        let row = self
            .local_sessions
            .as_ref()
            .and_then(|harnesses| harnesses.sessions.row(session_id));
        // What it is waiting for, before what it is: an operator who opened this
        // pane because a row was blinking should not have to read the screen to
        // find out why. Dropped once attached — they are in it now, and the
        // prompt itself is a foot below the title.
        let waiting = match (&row, attached) {
            (Some(row), false) if row.state.is_running() => row.attention.clone(),
            _ => None,
        };
        let what = match &row {
            // Who holds it goes in the title because it is the fact that
            // decides whether a prompt typed here will be interleaved with the
            // orchestrator's — and it is invisible everywhere else once the
            // cursor is on a lane rather than on the harness's own rail row.
            Some(row) => format!(
                "{} · {} · {}",
                row.provider.as_str(),
                row.state.as_str(),
                row.control.as_str()
            ),
            // A session that vanished between resolving and drawing. Rare, and
            // naming it beats a title that claims a provider we no longer know.
            None => "session".to_string(),
        };
        if attached {
            format!("{what} · typing here · {FOCUS_CHORD_LABEL} to release")
        } else if let Some(cue) = waiting {
            format!(
                "{what} · {ATTENTION_GLYPH} {} · Enter or {FOCUS_CHORD_LABEL} to answer",
                cue.label(medulla::clock::now_millis())
            )
        } else {
            format!("{what} · Enter or {FOCUS_CHORD_LABEL} to type")
        }
    }
}
