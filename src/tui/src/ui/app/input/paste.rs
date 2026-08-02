//! Where a bracketed-paste payload lands.
//!
//! Bracketed paste is on for the TUI's own terminal, so a paste no longer
//! arrives as synthetic key presses anywhere: it is one `Event::Paste`, and
//! every surface that accepts text has to be named here or the payload is
//! silently dropped. That makes this module the paste half of
//! [`super::super::keys`], and it follows that module's precedence exactly.

use crate::ui::composer::{insert_at, normalize_paste};

use super::super::types::App;

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
        if self.tab() != "Agents" || self.overlay_owns_keys() {
            return;
        }
        self.draft = insert_at(&self.draft.text, self.draft.cursor, &normalize_paste(text));
        // Narrowing the command peek invalidates where its cursor pointed,
        // exactly as typing a character does.
        self.command_index = 0;
    }
}
