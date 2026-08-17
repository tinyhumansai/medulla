//! Clearing the state one frame leaves behind for the next one.
//!
//! A draw records where things landed — which session the pane is showing, which
//! rectangles the mouse can hit — and the *next* key or click reads it back. That
//! only stays honest if every frame re-earns it: a pointer left over from the
//! last draw answers for a pane that may no longer be on screen. So each of these
//! is cleared here, at the top of the draw, and filled back in by whichever tab
//! actually paints it.

use super::super::types::App;

impl App {
    /// Drop everything the previous frame recorded, before this one draws.
    pub(super) fn reset_frame_state(&mut self) {
        // The harness pane is resolved during the draw and read by the *next*
        // key press, so it has to be cleared here rather than left over: a
        // `Ctrl-]` on the Settings tab must not attach to whatever the Agents
        // tab was showing several frames ago. `draw_agents_pane` fills it back
        // in when it resolves a session.
        self.pane_session = None;
        // Its counterpart, for the same reason: a remembered remote row would
        // answer the take chord on a tab that is not showing it.
        self.pane_remote_session = None;
        // Same reasoning as above: a stale rect would route the wheel into a
        // terminal that is no longer on screen.
        self.hit_session = None;
        self.hit_workflow_preview = None;
        // Focus follows the pane, not the other way round. `agents_selection`
        // (called only while drawing the Agents tab) is what notices the cursor
        // moving off the attached session; it has nothing to say once the
        // operator has left the tab entirely. Without this, `harness_focus`
        // stayed `Attached` after a click elsewhere, and the next keystroke —
        // meant for whatever tab was now on screen — was typed into a harness
        // pane the operator could no longer see.
        if self.harness_focus.attached_to().is_some() && self.tab() != "Agents" {
            self.release_session();
        }
    }
}
