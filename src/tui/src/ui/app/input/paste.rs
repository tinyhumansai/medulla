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
    /// operator never sees again. In order:
    ///
    /// 1. the hand-back question, asked while still attached — the chrome holds
    ///    the keyboard until it is answered, and its note takes the paste once
    ///    `E` has made that a text input;
    /// 2. the attached harness, which owns the keyboard outright and therefore
    ///    owns the paste, delivered to the child as a paste rather than as the
    ///    keystrokes it happens to spell;
    /// 3. an open inline prompt, flattened to one line because that is all it
    ///    can draw;
    /// 4. the session picker, whose workspace step is a path box and whose
    ///    harness step is a list;
    /// 5. any remaining modal, before anything per-tab — one can be raised over
    ///    a tab the operator has since moved to;
    /// 6. the Workflows copilot, when it is the focused pane;
    /// 7. the Agents composer, when one is actually on screen, with `\r\n` and
    ///    bare `\r` normalised to `\n`.
    ///
    /// Anything else drops the payload, and does so from an explicit arm: a
    /// surface that owns the keyboard and holds no visible field must swallow a
    /// paste rather than let it land in a draft behind it, because text retained
    /// out of sight is text the operator submits by accident later.
    ///
    /// Every arm is a `return`, so adding a surface here means deciding what it
    /// does with a paste rather than inheriting whatever the next branch does.
    pub(super) fn on_paste(&mut self, text: &str) {
        // An armed harness kill outranks everything, exactly as in [`App::on_key`]:
        // the confirmation owns one input and only a deliberate `y` proceeds. A
        // paste is an input, and a pasted "y" is not a deliberate one — so this
        // cancels rather than confirms, and the payload goes no further. Leaving
        // it out let a paste land in a composer while the question stayed armed
        // for whatever key came next.
        if self.kill_armed.take().is_some() {
            self.set_status("Session kill cancelled");
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
        // The session picker is two overlays in one: a harness-type list with no
        // field, then a visible path box. Routed as one call so the distinction
        // stays with the picker rather than being re-derived here.
        if self.agent_picker.is_some() {
            self.paste_into_harness_workspace(text);
            return;
        }
        // Every remaining modal — the resume picker and the decisions board —
        // owns the keyboard and offers no field, so it swallows the paste for
        // the same reason it swallows a keystroke. Checked here rather than
        // beside the composer because a modal can be raised over *any* tab: a
        // `/resume` answers asynchronously, so its picker lands over whichever
        // tab the operator moved to while it was in flight, and a check that ran
        // after the per-tab routing let a paste into the copilot underneath it.
        //
        // The arms above are still named in `overlay_owns_keys` on purpose: it
        // is one list, so an overlay added later cannot own the keyboard while a
        // paste lands behind it.
        if self.overlay_owns_keys() {
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
        // Every remaining tab is a list or a board with no text field of its
        // own, so a paste made on one is dropped rather than banked into the
        // Agents composer it is not looking at.
        if self.tab() != "Agents" {
            return;
        }
        if !self.agents_composer_shown() {
            self.set_status("Select a non-running session row to paste an instruction");
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
