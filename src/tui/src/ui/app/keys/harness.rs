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
//! everywhere else Enter keeps its normal meaning. Recognising the chord is
//! [`is_focus_chord`](crate::ui::harness_pane::keys::is_focus_chord) rather than
//! a character comparison — terminals do not deliver it the way it is written.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::harness_pane::{
    keys::{encode, is_focus_chord},
    HarnessFocus, FOCUS_CHORD_LABEL,
};
use crate::worker::pty::HarnessControl;

use super::super::types::App;

impl App {
    /// Take the keyboard back from whatever harness has it.
    ///
    /// Safe to call when nothing is attached; that is the common case on the
    /// render path, which calls this whenever the selection moves.
    pub(crate) fn release_harness(&mut self) {
        // The operator has been looking at and handling this pane. Consume any
        // completion bell observed while it was attached so detaching cannot
        // reveal a stale alert that the hidden rail deliberately suppressed.
        if let (Some(session), Some(harnesses)) =
            (self.harness_focus.attached_to(), self.harnesses.as_ref())
        {
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
                // Releasing the keyboard is also the moment to settle who holds
                // the harness. `begin_harness_release` answers `false` when it
                // opened a prompt about that, and the keyboard must stay put
                // until it is answered — moving it out from under the question
                // would leave the operator answering about a pane they can no
                // longer see the state of.
                if self.begin_harness_release(&session) {
                    self.release_harness();
                    // The keyboard has to land somewhere it can be seen. The
                    // cursor is on a harness row, which draws no composer, so
                    // the rail is the only half of the tab that can answer a
                    // keystroke — leaving focus on the composer is what made
                    // every key after a release look like a dead terminal.
                    self.focus_agents_rail();
                    self.set_status(format!(
                        "Released the harness · {FOCUS_CHORD_LABEL} to type again"
                    ));
                }
                return true;
            }
            self.type_into_harness(&session, key);
            return true;
        }
        // *Attaching* is a chrome binding, not a mode, so it yields to whatever
        // overlay is on top of the chrome. The pane behind an open picker is
        // still drawn — and so still resolves a harness session — which is how
        // Enter in the "start a harness" modal used to attach to the harness
        // already selected underneath it instead of launching the chosen one.
        if self.overlay_owns_keys() {
            return false;
        }
        let enter_on_harness = key.code == KeyCode::Enter
            && key.modifiers == KeyModifiers::NONE
            && self.harness_pane_session.is_some();
        if is_focus_chord(key) || enter_on_harness {
            self.attach_to_pane_harness();
            // Consumed either way. The chord is reserved, while Enter reaches
            // this branch only when the visible pane resolved to a harness; it
            // must not submit a hidden composer or return focus to one.
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
    pub(in crate::ui::app) fn attach_to_pane_harness(&mut self) {
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
        // Focusing in *is* taking over. Keyboard ownership without control is
        // the trap this whole feature exists to close: the orchestrator's next
        // task frame would be pasted into the composer the operator is typing
        // in, and a harness serves one turn at a time, so the two prompts come
        // back as one confidently wrong answer rather than as an error.
        let took = self
            .harnesses
            .as_ref()
            .and_then(|harnesses| harnesses.control(&session))
            == Some(HarnessControl::Orchestrator);
        if took {
            if let Some(harnesses) = self.harnesses.clone() {
                harnesses.set_control(&session, HarnessControl::User);
            }
            self.harness_took_control = true;
        }
        // Attaching answers whatever the harness was blinking about: the
        // operator is now looking at the screen that was asking. A named prompt
        // that is still up returns on the next refresh, so nothing is lost by
        // clearing it here — and a rail that keeps blinking at the pane you are
        // already typing in is how an indicator becomes furniture.
        if let Some(harnesses) = self.harnesses.as_ref() {
            harnesses.sessions.acknowledge(&session);
        }
        self.harness_focus = HarnessFocus::Attached(session);
        self.set_status(format!(
            "Typing into the harness · you have control · {FOCUS_CHORD_LABEL} to release"
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
            // The child died between the last frame and this keystroke. Give
            // the session back on the way out: a dead harness left under user
            // control is a slot nothing can ever reclaim, and there is nobody
            // left to answer a hand-back prompt about it.
            if self.harness_took_control {
                harnesses.set_control(session, HarnessControl::Orchestrator);
                self.harness_took_control = false;
            }
            self.release_harness();
            self.set_status(format!("Harness stopped listening ({err})"));
            return;
        }
        // Typing means "I am here now". A pane left scrolled back would keep
        // showing history while the harness answered below it, unseen. No-op
        // for a harness that owns its own scrollback.
        harnesses.scroll_to_live(session);
    }
}
