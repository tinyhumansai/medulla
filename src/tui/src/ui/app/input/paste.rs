//! Where a bracketed-paste payload lands.
//!
//! Bracketed paste is on for the TUI's own terminal, so a paste no longer
//! arrives as synthetic key presses anywhere: it is one [`Event::Paste`], and
//! every surface that accepts text has to be named here or the payload is
//! silently dropped. That makes this module the paste half of
//! [`super::super::keys`], and it follows that module's precedence exactly.

use crate::ui::composer::{insert_at, normalize_paste};

use super::super::types::App;
#[cfg(feature = "workflows")]
use super::super::types::WorkflowFocus;

impl App {
    /// Insert a bracketed-paste payload into whatever text field has the caret.
    ///
    /// The point of routing paste as its own event is that the payload is never
    /// re-read as key presses: a newline inside it lands in the draft instead of
    /// reaching the `Enter` bindings, so a multi-line paste no longer submits
    /// itself — once per line, at that — and a `/`-prefixed paste no longer runs
    /// the command the peek happens to be highlighting. Only an explicit `Enter`
    /// submits.
    ///
    /// Who takes it follows [`App::on_key`]'s precedence exactly, because a
    /// paste that landed somewhere the keyboard is not would be a payload the
    /// operator never sees again. So: the hand-back question first, since it is
    /// asked while still attached and the chrome holds the keyboard until it is
    /// answered; then the attached harness, which owns the keyboard outright and
    /// therefore owns the paste; then an open inline prompt, flattened to one
    /// line because that is all it can draw. Otherwise the Agents composer takes
    /// it, with `\r\n` and bare `\r` normalised to `\n`. Modals that own no text
    /// field (the harness and resume pickers, the decisions overlay) swallow the
    /// paste for the same reason they swallow the keyboard.
    pub(super) fn on_paste(&mut self, text: &str) {
        if self.handback_prompt.is_some() {
            return;
        }
        if let Some(session) = self.harness_focus.attached_to().map(str::to_string) {
            self.paste_into_harness(&session, text);
            return;
        }
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.paste(text);
            return;
        }
        // The harness picker is two overlays in one: a provider list with no
        // field, then a visible path box. Routed as one call so the distinction
        // stays with the picker rather than being re-derived here.
        if self.harness_picker.is_some() {
            self.paste_into_harness_workspace(text);
            return;
        }
        // The Workflows copilot is the app's other multiline composer, and the
        // pane a long instruction is most likely to be pasted into. The
        // catalogue and the canvas beside it are lists: they own the keyboard
        // for their focus and hold no field a payload could be seen in, so a
        // paste made on either is dropped rather than banked into the copilot
        // one pane over.
        #[cfg(feature = "workflows")]
        if self.tab() == "Workflows" {
            if self.wf.focus == WorkflowFocus::Copilot {
                self.wf.draft = insert_at(
                    &self.wf.draft.text,
                    self.wf.draft.cursor,
                    &normalize_paste(text),
                );
            }
            return;
        }
        if self.tab() != "Agents" || self.overlay_owns_keys() {
            return;
        }
        // The composer belongs to the orchestrator lane and nowhere else, so a
        // paste made with the cursor on a worker or an unattached harness row
        // has nowhere visible to land. Refused with a reason rather than
        // retained: text held in a draft nobody can see is text the operator
        // submits by accident on their next trip back to the orchestrator.
        if !self.agents_composer_shown() {
            self.set_status("Select the orchestrator lane to paste an instruction");
            return;
        }
        // Pasting is typing, so the keyboard follows it in — exactly as a
        // printable key does from the rail. Leaving focus behind made the next
        // Enter step back into the composer instead of sending what was just
        // pasted, and the arrows keep walking lanes over text nobody can edit.
        self.focus_agents_composer();
        self.draft = insert_at(&self.draft.text, self.draft.cursor, &normalize_paste(text));
        // Narrowing the command peek invalidates where its cursor pointed,
        // exactly as typing a character does.
        self.command_index = 0;
    }
}
