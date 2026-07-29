//! Handing the keyboard to an embedded harness, and taking it back.
//!
//! While attached, the orchestrator's own bindings are *gone*: `q` does not
//! quit, Tab does not change tabs, Escape does not close anything. That is the
//! point — the operator is typing into Claude Code or Codex, and a wrapper that
//! kept a handful of keys for itself would be a wrapper that intercepts exactly
//! the keys the harness needs at the moment it needs them.
//!
//! The single exception is the focus chord, `Ctrl-]`. One reserved key is the
//! minimum that leaves a way out, and it is the traditional choice precisely
//! because full-screen programs do not bind it. Recognising it is
//! [`is_focus_chord`](crate::ui::harness_pane::keys::is_focus_chord) rather than
//! a character comparison — terminals do not deliver it the way it is written.

use crossterm::event::KeyEvent;

use crate::ui::harness_pane::{
    keys::{encode, is_focus_chord},
    HarnessFocus, FOCUS_CHORD_LABEL,
};

use super::super::types::App;

impl App {
    /// Take the keyboard back from whatever harness has it.
    ///
    /// Safe to call when nothing is attached; that is the common case on the
    /// render path, which calls this whenever the selection moves.
    pub(crate) fn release_harness(&mut self) {
        self.harness_focus = HarnessFocus::Chrome;
    }

    /// Route a key press while a harness owns the keyboard, or when the chord
    /// asks it to.
    ///
    /// Returns `true` when the key was consumed and the tab bindings must not
    /// see it. Called first in [`App::on_key`] — before overlays, before global
    /// chords — because "attached" is a mode, and a mode that yields to a
    /// shortcut is not one.
    pub(super) fn handle_harness_key(&mut self, key: KeyEvent) -> bool {
        if let Some(session) = self.harness_focus.attached_to().map(str::to_string) {
            if is_focus_chord(key) {
                self.release_harness();
                self.set_status(format!(
                    "Released the harness · {FOCUS_CHORD_LABEL} to type again"
                ));
                return true;
            }
            self.type_into_harness(&session, key);
            return true;
        }
        if is_focus_chord(key) {
            self.attach_to_pane_harness();
            // Consumed either way: the chord is reserved, so it must not fall
            // through to a tab binding just because there was nothing to attach
            // to. Falling through would make `Ctrl-]` mean different things
            // depending on what the cursor happened to be on.
            return true;
        }
        false
    }

    /// Attach to the harness the Agents pane resolved on the last draw.
    ///
    /// Refuses, with a reason, rather than silently doing nothing: an operator
    /// who pressed the chord and saw no change has no way to tell "wrong row"
    /// from "the feature is broken".
    fn attach_to_pane_harness(&mut self) {
        let Some(session) = self.harness_pane_session.clone() else {
            self.set_status("No harness on this row — select a running task to type into one");
            return;
        };
        let running = self
            .harnesses
            .as_ref()
            .is_some_and(|harnesses| harnesses.is_running(&session));
        if !running {
            self.set_status("That harness has exited — its last screen is all that is left");
            return;
        }
        self.harness_focus = HarnessFocus::Attached(session);
        self.set_status(format!(
            "Typing into the harness · {FOCUS_CHORD_LABEL} to release the keyboard"
        ));
    }

    /// Encode one key press and write it to the attached harness's PTY.
    ///
    /// A write failure means the child died between the last frame and this
    /// keystroke. Detaching on it is what keeps the operator from typing into a
    /// pane that stopped listening several keys ago.
    fn type_into_harness(&mut self, session: &str, key: KeyEvent) {
        // A key with no terminal form (a bare modifier press) is not an error —
        // a real terminal transmits nothing for it either.
        let Some(bytes) = encode(key) else {
            return;
        };
        let Some(harnesses) = self.harnesses.clone() else {
            self.release_harness();
            return;
        };
        if let Err(err) = harnesses.write(session, &bytes) {
            self.release_harness();
            self.set_status(format!("Harness stopped listening ({err})"));
        }
    }
}
