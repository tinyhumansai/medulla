//! Who is in front of the content pane, and therefore who owns input.
//!
//! Two questions have to agree about overlays: "what does [`App::draw`] paint on
//! top?" and "who owns the keyboard and the clipboard?". They were answered by
//! two separately maintained conditions, and they drifted twice — first the
//! resume picker, then the agent-template popup, each of which was rendered over
//! a composer that went on quietly accepting pastes behind it.
//!
//! So the answer lives here once. [`App::visible_overlays`] is the single source
//! of truth: the render iterates it to decide what to paint, and
//! [`App::overlay_owns_keys`] answers from it. Adding an overlay means adding one
//! variant to [`Overlay`] and one predicate here, and both surfaces pick it up —
//! there is no second place left to forget.

use super::types::{App, Overlay};

impl App {
    /// Every overlay currently on screen, in the order the render stacks them.
    ///
    /// Each entry pairs a variant with the condition that puts it on screen, so
    /// "is it drawn?" and "does it own input?" cannot be answered differently.
    /// One subtlety is encoded here rather than at the call sites: the resume
    /// picker and the inline prompt share the row below the content, and the
    /// prompt wins, so the picker is only on screen when no prompt is.
    pub(super) fn visible_overlays(&self) -> Vec<Overlay> {
        [
            (Overlay::Decisions, self.decision_open),
            (Overlay::SessionPicker, self.session_picker.is_some()),
            (
                Overlay::SessionKill,
                self.kill_armed.is_some() || self.harness_close_armed.is_some(),
            ),
            (
                Overlay::WorkflowDelete,
                self.workflow_delete_armed.is_some(),
            ),
            (Overlay::InlinePrompt, self.prompt.is_some()),
            (
                Overlay::ResumePicker,
                self.resume_picker.is_some() && self.prompt.is_none(),
            ),
        ]
        .into_iter()
        .filter_map(|(overlay, shown)| shown.then_some(overlay))
        .collect()
    }

    /// Whether anything is drawn in front of the content pane.
    ///
    /// The paste router's and the attach chord's answer to "is there something
    /// between the operator and the pane they think they are typing into?".
    /// Derived from [`App::visible_overlays`] so it cannot fall behind the
    /// render: an overlay that is painted over a composer must not leave that
    /// composer taking input nobody can see going in.
    ///
    /// Note that the overlays which *do* own a text field — the inline prompt
    /// and the picker's workspace query — are routed to before
    /// this is consulted, so answering `true` for them is not a contradiction.
    pub(in crate::ui::app) fn overlay_owns_keys(&self) -> bool {
        !self.visible_overlays().is_empty()
    }
}
