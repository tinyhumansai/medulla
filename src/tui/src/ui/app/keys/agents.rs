//! Agents-tab focus: which half of the surface the keyboard is driving, and the
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

use super::super::types::{AgentsFocus, App, Cmd, PaneView};
use crate::ui::composer::insert_at;

/// What rail-focus handling did with one key press.
pub(in crate::ui::app) enum AgentsKey {
    /// The key belonged to the rail; any follow-up command is carried along.
    Handled(Option<Cmd>),
    /// The rail does not claim this key — global and composer handling apply.
    Unhandled,
}

impl App {
    /// Whether a composer is actually drawn for the row under the cursor.
    ///
    /// It is available whenever the selected row is not a live harness. Both
    /// conditions are the ones
    /// [`agents_panes`](crate::ui::app::render) lays out from, so the keyboard
    /// and the screen cannot disagree about whether there is somewhere to type.
    pub fn agents_composer_shown(&self) -> bool {
        self.pane_session.is_none()
    }

    /// Whether the Agents rail currently holds the keyboard.
    ///
    /// The rail takes it unconditionally when no composer is drawn. Without
    /// that, stepping out of an attached harness — or simply arrowing onto a
    /// harness row — left focus on a composer that is not on screen: arrows
    /// drove an invisible caret and characters landed in a draft nobody could
    /// see, which reads exactly like the keyboard having died.
    pub fn agents_rail_focused(&self) -> bool {
        self.agents_focus == AgentsFocus::Rail || !self.agents_composer_shown()
    }

    /// Whether the rail cursor sits on the `+ New agent` action row.
    pub(in crate::ui::app) fn on_new_agent_row(&self) -> bool {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        rows.get(self.rail_cursor_in(&rows, &lanes))
            .is_some_and(|row| row.is_new_agent())
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

    /// The agent whose `+ new session` action row the cursor sits on, if it does.
    pub(in crate::ui::app) fn on_new_session_row(&self) -> Option<String> {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        rows.get(self.rail_cursor_in(&rows, &lanes))
            .and_then(|row| row.new_session_agent())
            .map(str::to_string)
    }

    /// Move the keyboard to the rail. Nothing else about the draft changes, so
    /// stepping out to look at a lane and back never costs a half-typed message.
    pub(in crate::ui::app) fn focus_agents_rail(&mut self) {
        self.agents_focus = AgentsFocus::Rail;
    }

    /// Move the keyboard back to the composer.
    pub(in crate::ui::app) fn focus_agents_composer(&mut self) {
        self.agents_focus = AgentsFocus::Composer;
    }

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
    /// Returns [`AgentsKey::Unhandled`] for anything the rail has no opinion on
    /// — tab switching, transcript paging, the `Alt` steering chords — so those
    /// keep working identically from either side of the tab.
    // Visible to the whole `ui::app` module, not just `keys`: the rail's own
    // tests drive it from `session_control_tests`, one level up.
    pub(in crate::ui::app) fn on_agents_rail_key(&mut self, k: KeyEvent) -> AgentsKey {
        if !self.agents_rail_focused() {
            return AgentsKey::Unhandled;
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
            return AgentsKey::Handled(None);
        }

        match k.code {
            // A harness row has an immutable launch snapshot, so `d` shows what
            // this session has changed since it launched — in the pane the
            // terminal was in, because that is the row the question was asked
            // from. When no harness is shown, `d` remains ordinary typing.
            KeyCode::Char('d') if !ctrl && !alt && self.pane_session.is_some() => {
                self.toggle_harness_diff_pane();
                AgentsKey::Handled(None)
            }
            // `D` is the same diff on the Changes tab: the pane is half a
            // screen, and a review with comments on it wants the whole one.
            KeyCode::Char('D') if !ctrl && !alt && self.pane_session.is_some() => {
                AgentsKey::Handled(self.open_selected_harness_changes())
            }
            // `k` closes the harness the pane is showing — the other half of the
            // two things an operator wants from a session they are looking at
            // but not typing into. It asks first; see `close_pane_session_prompt`.
            KeyCode::Char('k') if !ctrl && !alt && self.pane_session.is_some() => {
                self.close_pane_session_prompt();
                AgentsKey::Handled(None)
            }
            KeyCode::Char('K') => {
                if let Some(target) = self.kill_target() {
                    self.arm_kill(target);
                } else {
                    self.set_status("Select a running session first");
                }
                AgentsKey::Handled(None)
            }
            // The bare arrows are the point of having focus at all.
            KeyCode::Up | KeyCode::Down => {
                self.agent_scroll = 0;
                self.chat_scroll = 0;
                self.move_agent_index(matches!(k.code, KeyCode::Up));
                // Arrowing onto a task watches it, exactly as clicking does.
                AgentsKey::Handled(self.retarget_watch())
            }
            // Enter is "I have found the row I wanted; let me type" — except on
            // the rows that are themselves an action: `+ New agent`, an agent's
            // `+ new session`, and a lane's `+N more`, where it pages the hidden
            // sessions into view. A visible session pane consumes it earlier and
            // takes the keyboard instead.
            KeyCode::Enter => {
                #[cfg(feature = "workflows")]
                let workflow_run = self.on_workflow_run_row();
                #[cfg(not(feature = "workflows"))]
                let workflow_run: Option<(String, String)> = None;
                if self.on_new_agent_row() {
                    self.open_new_agent_picker();
                } else if let Some((workflow, run)) = workflow_run {
                    // A run row exists to be followed: the rail is where the
                    // operator learns the session started one, and the graph is
                    // where they find out what it is doing.
                    self.follow_workflow_run(&workflow, &run);
                } else if let Some(agent_id) = self.on_new_session_row() {
                    // The same flow `Ctrl-T` opens on an agent row: the name
                    // prompt, then a session in that agent's declared harness
                    // and directory. Enter is how a row that *is* an action is
                    // taken, so the two doors lead to one place.
                    self.open_new_session(&agent_id);
                } else if !self.page_subtasks() {
                    self.focus_agents_composer();
                }
                AgentsKey::Handled(None)
            }
            // Esc returns focus to the composer, so the key that moved focus out
            // also brings it back.
            KeyCode::Esc => {
                self.focus_agents_composer();
                AgentsKey::Handled(None)
            }
            // Typing goes to the conversation, from whatever row the cursor is
            // on. The composer is drawn for the orchestrator lane and nowhere
            // else, so anywhere else this moves the cursor there first — the
            // character is what says where it was meant to go.
            //
            // It used to be refused instead, with a status line telling the
            // operator to go and select that lane by hand. That left the only
            // text box in the tab reachable only by arrowing to one specific
            // row, and threw the keystroke away on the way. Focus still may not
            // move to a composer that is not drawn (that was the older bug: an
            // invisible input swallowing the character and every arrow after
            // it), which is exactly why the cursor moves rather than the focus
            // alone.
            KeyCode::Char(c) if !ctrl && !alt => {
                self.focus_agents_composer();
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
                self.command_index = 0;
                AgentsKey::Handled(None)
            }
            // Backspace is typing too — it edits the draft it belongs to, and
            // only where that draft is on screen.
            KeyCode::Backspace if self.agents_composer_shown() => {
                self.focus_agents_composer();
                AgentsKey::Unhandled
            }
            _ => AgentsKey::Unhandled,
        }
    }
}
