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
//! because full-screen programs do not bind it. While the chrome owns the
//! keyboard, plain Enter also attaches when the selected pane is a harness;
//! everywhere else Enter keeps its normal meaning — including under an open
//! overlay, which owns the keyboard until it is answered. Recognising the chord is
//! [`is_focus_chord`](crate::ui::harness_pane::keys::is_focus_chord) rather than
//! a character comparison — terminals do not deliver it the way it is written.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::harness_pane::{
    keys::{encode, is_focus_chord},
    HarnessFocus, FOCUS_CHORD_LABEL,
};
use crate::worker::pty::launch::bracket_paste;
use crate::worker::pty::SessionControl;

use super::super::types::{App, PaneView};

impl App {
    /// Put the harness's own terminal back in the pane.
    ///
    /// A no-op when it is already there, which is the common case: every path
    /// that hands the keyboard to a harness calls this first rather than
    /// checking, because "the pane shows what the keys reach" is an invariant,
    /// not a special case.
    pub(in crate::ui::app) fn show_harness_pane(&mut self) {
        self.pane_view = PaneView::Harness;
    }

    /// Take the keyboard back from whatever harness has it.
    ///
    /// Safe to call when nothing is attached; that is the common case on the
    /// render path, which calls this whenever the selection moves.
    pub(crate) fn release_session(&mut self) {
        // The operator has been looking at and handling this pane. Consume any
        // completion bell observed while it was attached so detaching cannot
        // reveal a stale alert that the hidden rail deliberately suppressed.
        if let (Some(session), Some(harnesses)) = (
            self.harness_focus.attached_to(),
            self.local_sessions.as_ref(),
        ) {
            harnesses.sessions.acknowledge(session);
        }
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
                self.release_session();
                self.focus_agents_rail();
                self.set_status(format!(
                    "Released the session · {FOCUS_CHORD_LABEL} to type again"
                ));
                return true;
            }
            self.type_into_harness(&session, key);
            return true;
        }
        // *Attaching* is a chrome binding, not a mode, so it yields to whatever
        // overlay is on top of the chrome. The pane behind an open picker is
        // still drawn — and so still resolves a harness session — which is how
        // Enter in the "start a session" modal used to attach to the session
        // already selected underneath it instead of launching the chosen one.
        if self.overlay_owns_keys() {
            return false;
        }
        // `agents_rail_focused`, not the raw focus field. A harness row draws no
        // composer, so the rail is driving the keyboard whether or not the
        // operator ever pressed Esc to say so — and the field alone is `Composer`
        // for anyone who arrived by `Alt`+`↓` or by clicking. Enter then fell
        // through to the composer bindings, which moved focus to an input that
        // is not on screen; the *next* Enter submitted an empty turn. Two
        // keystrokes into a pane that plainly says "Enter to type", and nothing
        // had happened.
        let enter_on_harness = key.code == KeyCode::Enter
            && key.modifiers == KeyModifiers::NONE
            && self.pane_view == PaneView::Harness
            && self.pane_session.is_some()
            && self.agents_rail_focused();
        if enter_on_harness {
            // Typing into a harness means looking at it. If the pane was showing
            // the session's diff instead, the terminal comes back first —
            // keystrokes landing in a screen that is not on the screen is the
            // one failure the whole focus model exists to prevent.
            self.show_harness_pane();
            self.attach_to_pane_session();
            // Consumed either way: Enter reaches this branch only when the
            // visible pane resolved to a harness, so it must not submit a
            // hidden composer or return focus to one.
            return true;
        }
        if is_focus_chord(key) {
            self.attach_to_pane_session();
            return true;
        }
        false
    }

    /// Attach to the harness the Agents pane resolved on the last draw.
    ///
    /// Refuses, with a reason, rather than silently doing nothing: an operator
    /// who pressed the chord and saw no change has no way to tell "wrong row"
    /// from "the feature is broken".
    ///
    /// Reachable from the pointer as well as from the chord: clicking into a
    /// terminal is what every other terminal on the machine means by "type
    /// here", and requiring a chord to do it made the embedded pane the one
    /// exception.
    pub(in crate::ui::app) fn attach_to_pane_session(&mut self) {
        // Same reason as the Enter path: the keyboard may only go somewhere the
        // operator can see it land.
        self.show_harness_pane();
        let Some(session) = self.pane_session.clone() else {
            self.set_status("No session on this row — select a running one to type into");
            return;
        };
        self.attach_to_session(&session);
    }

    /// Attach to one named operator-owned session.
    ///
    /// Split from [`attach_to_pane_session`](Self::attach_to_pane_session) so
    /// callers that already know which session they mean — the takeover question
    /// being answered, the rail row that was clicked — do not have to round-trip
    /// through the cursor to say so.
    pub(in crate::ui::app) fn attach_to_session(&mut self, session: &str) {
        let running = self
            .local_sessions
            .as_ref()
            .is_some_and(|harnesses| harnesses.is_running(session));
        if !running {
            self.set_status("That session has exited — its last screen is all that is left");
            return;
        }
        let control = self
            .local_sessions
            .as_ref()
            .and_then(|harnesses| harnesses.control(session));
        if control == Some(SessionControl::Orchestrator) {
            self.set_status("Task sessions are view-only while they are running");
            return;
        }
        // Attaching answers whatever the harness was blinking about: the
        // operator is now looking at the screen that was asking. A named prompt
        // that is still up returns on the next refresh, so nothing is lost by
        // clearing it here — and a rail that keeps blinking at the pane you are
        // already typing in is how an indicator becomes furniture.
        if let Some(harnesses) = self.local_sessions.as_ref() {
            harnesses.sessions.acknowledge(session);
        }
        self.harness_focus = HarnessFocus::Attached(session.to_string());
        self.set_status(format!(
            "Typing into the session · you have control · {FOCUS_CHORD_LABEL} to release"
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
        self.write_to_harness(session, &bytes);
    }

    /// Hand a bracketed-paste payload to the attached harness.
    ///
    /// The attached pane *is* the operator's terminal, so a paste has to reach
    /// the child the way its own terminal would deliver one: as a paste, not as
    /// the keystrokes it happens to spell. Replaying it as keys is what makes a
    /// multi-line clipboard submit itself once per line inside Claude Code.
    ///
    /// How it is encoded is the child's choice, not ours — the same rule the
    /// wheel follows for mouse reports. A child that turned bracketed paste on
    /// gets the markers and takes the whole payload as one block; one that never
    /// asked gets the bytes alone, because `ESC[200~` sent to it arrives as an
    /// escape key press followed by literal text. Line breaks become `\r` in
    /// that case for the same reason [`encode`] maps Enter to it: the child's
    /// line discipline is raw and reads carriage return as the end of a line.
    pub(in crate::ui::app) fn paste_into_harness(&mut self, session: &str, text: &str) {
        let bracketed = self
            .local_sessions
            .as_ref()
            .and_then(|harnesses| harnesses.sessions.bracketed_paste(session))
            .unwrap_or(false);
        let bytes = if bracketed {
            bracket_paste(text)
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.write_to_harness(session, &bytes);
    }

    /// Write already-encoded bytes to the attached harness, detaching if the
    /// child has stopped listening.
    fn write_to_harness(&mut self, session: &str, bytes: &[u8]) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.release_session();
            return;
        };
        if let Err(err) = harnesses.write(session, bytes) {
            self.release_session();
            self.set_status(format!("Session stopped listening ({err})"));
            return;
        }
        // Typing means "I am here now". A pane left scrolled back would keep
        // showing history while the harness answered below it, unseen. No-op
        // for a harness that owns its own scrollback.
        harnesses.scroll_to_live(session);
    }
}
