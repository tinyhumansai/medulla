//! Sessions-tab focus: which half of the surface the keyboard is driving, and the
//! bindings that apply while the rail holds it.
//!
//! The tab merges a list with a text input, and a terminal has one keyboard for
//! both. The composer wins by default — typing has to work the instant the tab
//! opens — which historically left the rail reachable only through
//! `Alt`+`↑`/`↓`. That binding is unreachable on a stock macOS terminal, where
//! Option+Arrow is either swallowed or sent as a word-motion sequence, so on
//! those machines there was no way to select anything but the orchestrator.
//!
//! Focus is therefore explicit, and moved with keys every terminal can send:
//! `Esc` steps out of the composer to the rail, `Enter` steps back, and typing
//! anywhere returns to the composer with the character intact. When the
//! selected row resolves to an embedded harness, Enter instead attaches to its
//! terminal. This mirrors the menu/content model
//! [`multi_pane`](crate::ui::multi_pane) already gives Settings and Routing.
//! `Alt`+`↑`/`↓` still works for anyone whose terminal sends it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::types::{App, Cmd, PaneView};

/// What rail-focus handling did with one key press.
pub(in crate::ui::app) enum SessionsKey {
    /// The key belonged to the rail; any follow-up command is carried along.
    Handled(Option<Cmd>),
    /// The rail does not claim this key — global and composer handling apply.
    Unhandled,
}

impl App {
    /// Whether the Sessions rail currently holds the keyboard.
    ///
    /// Always, now. The tab used to be split between the rail and the
    /// orchestrator's composer, and focus had to be tracked because a terminal
    /// has one keyboard for both. There is no composer any more — the
    /// orchestrator runs below the surface rather than being typed at — so the
    /// rail is the only thing on the tab that can hold the keyboard. Kept as a
    /// named predicate because the rail's panel border reads it to say so.
    pub fn sessions_rail_focused(&self) -> bool {
        true
    }

    /// Whether the rail cursor sits on the `+ New session` action row.
    pub(in crate::ui::app) fn on_new_session_row(&self) -> bool {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        rows.get(self.rail_cursor_in(&rows, &lanes))
            .is_some_and(|row| row.is_new_session())
    }

    /// The workflow run the rail cursor sits on, when it sits on one.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) fn on_workflow_run_row(&self) -> Option<(String, String)> {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        rows.get(self.rail_cursor_in(&rows, &lanes))
            .and_then(|row| row.workflow_run())
            .map(|row| (row.run.workflow_id.clone(), row.run.run_id.clone()))
    }

    /// Open the selected run in the Workflows tab when that feature is present.
    #[cfg(feature = "workflows")]
    fn follow_workflow_run(&mut self, workflow: &str, run: &str) {
        self.open_workflow_run(workflow, run);
    }

    /// A slim build still lists reported runs, but has no workflow tab to open.
    #[cfg(not(feature = "workflows"))]
    fn follow_workflow_run(&mut self, _workflow: &str, _run: &str) {}

    /// Handle a key while the pane is showing one of the session's non-terminal
    /// views. Returns `true` when the view claimed it.
    ///
    /// `d` and `Esc` both go back to the harness: one is the key that opened the
    /// view, the other is what every other overlay in the TUI closes with, and a
    /// view that replaced the whole pane needs an exit that is guessable.
    /// Everything else is delegated to the view's own bindings, which are the
    /// tab's — two copies of "how do I move down a diff" would drift.
    fn on_pane_view_key(&mut self, k: KeyEvent) -> bool {
        if k.modifiers.contains(KeyModifiers::CONTROL) || k.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        match self.pane_view {
            PaneView::Harness => false,
            PaneView::Diff if self.changes.picking_baseline => self.on_changes_key(k.code),
            PaneView::Diff => match k.code {
                KeyCode::Char('d') | KeyCode::Esc => {
                    self.toggle_harness_diff_pane();
                    true
                }
                code => self.on_changes_key(code),
            },
        }
    }

    /// Handle a key while the rail holds focus.
    ///
    /// Returns [`SessionsKey::Unhandled`] for anything the rail has no opinion on
    /// — tab switching, transcript paging, the `Alt` steering chords — so those
    /// keep working identically from either side of the tab.
    // Visible to the whole `ui::app` module, not just `keys`: the rail's own
    // tests drive it from `session_control_tests`, one level up.
    pub(in crate::ui::app) fn on_sessions_rail_key(&mut self, k: KeyEvent) -> SessionsKey {
        if !self.sessions_rail_focused() {
            return SessionsKey::Unhandled;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // A pane showing one of the session's other views binds that view's
        // keys, not the harness's: while the diff is up, `j`/`k` walk the patch
        // exactly as they do on the Changes tab. Anything the view does not
        // claim — Tab, the steering chords — falls through untouched.
        if self.pane_view != PaneView::Harness
            && self.pane_session.is_some()
            && (self.on_pane_view_key(k)
                // The diff replaces the composer. An unbound printable key must
                // stay with that visible view instead of falling through to the
                // rail's typing shortcut and silently changing a hidden draft.
                || (!ctrl && !alt && matches!(k.code, KeyCode::Char(_))))
        {
            return SessionsKey::Handled(None);
        }

        match k.code {
            // A harness row has an immutable launch snapshot, so `d` shows what
            // this session has changed since it launched — in the pane the
            // terminal was in, because that is the row the question was asked
            // from. When no harness is shown, `d` remains ordinary typing.
            KeyCode::Char('d') if !ctrl && !alt && self.pane_session.is_some() => {
                self.toggle_harness_diff_pane();
                SessionsKey::Handled(None)
            }
            // `D` is the same diff on the Changes tab: the pane is half a
            // screen, and a review with comments on it wants the whole one.
            KeyCode::Char('D') if !ctrl && !alt && self.pane_session.is_some() => {
                SessionsKey::Handled(self.open_selected_harness_changes())
            }
            // `k` closes the harness the pane is showing — the other half of the
            // two things an operator wants from a session they are looking at
            // but not typing into. It asks first; see `close_pane_session_prompt`.
            KeyCode::Char('k') if !ctrl && !alt && self.pane_session.is_some() => {
                self.close_pane_session_prompt();
                SessionsKey::Handled(None)
            }
            KeyCode::Char('K') => {
                if let Some(target) = self.kill_target() {
                    self.arm_kill(target);
                } else {
                    self.set_status("Select a running session first");
                }
                SessionsKey::Handled(None)
            }
            // The bare arrows are the point of having focus at all.
            KeyCode::Up | KeyCode::Down => {
                self.agent_scroll = 0;
                self.move_rail_index(matches!(k.code, KeyCode::Up));
                // Arrowing onto a task watches it, exactly as clicking does.
                SessionsKey::Handled(self.retarget_watch())
            }
            // Enter is "I have found the row I wanted; let me type" — except on
            // the rows that are themselves an action: the two `+` rows, which
            // open the session picker, and a lane's `+N more`, where it pages
            // the hidden sessions into view. A visible session pane consumes it
            // earlier and takes the keyboard instead.
            KeyCode::Enter => {
                #[cfg(feature = "workflows")]
                let workflow_run = self.on_workflow_run_row();
                #[cfg(not(feature = "workflows"))]
                let workflow_run: Option<(String, String)> = None;
                if self.on_new_session_row() {
                    // The same door `Ctrl-T` opens: harness, then directory,
                    // then the session is running.
                    self.open_session_picker();
                } else if let Some((workflow, run)) = workflow_run {
                    // A run row exists to be followed: the rail is where the
                    // operator learns the session started one, and the graph is
                    // where they find out what it is doing.
                    self.follow_workflow_run(&workflow, &run);
                } else {
                    // Nothing else to do: `Enter` on a session row is claimed
                    // earlier, by the pane that attaches the keyboard to it.
                    self.page_subtasks();
                }
                SessionsKey::Handled(None)
            }
            _ => SessionsKey::Unhandled,
        }
    }
}
