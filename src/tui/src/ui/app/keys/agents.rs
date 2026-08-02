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

use super::super::types::{AgentsFocus, App, Cmd};
use crate::ui::composer::insert_at;

/// What rail-focus handling did with one key press.
pub(super) enum AgentsKey {
    /// The key belonged to the rail; any follow-up command is carried along.
    Handled(Option<Cmd>),
    /// The rail does not claim this key — global and composer handling apply.
    Unhandled,
}

impl App {
    /// Whether a composer is actually drawn for the row under the cursor.
    ///
    /// It belongs to the orchestrator lane and nowhere else, and a harness takes
    /// the whole right column when one is on it. Both conditions are the ones
    /// [`agents_panes`](crate::ui::app::render) lays out from, so the keyboard
    /// and the screen cannot disagree about whether there is somewhere to type.
    pub fn agents_composer_shown(&self) -> bool {
        self.on_orchestrator_lane() && self.harness_pane_session.is_none()
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

    /// Whether the rail cursor sits on the `+ New harness` action row.
    pub(in crate::ui::app) fn on_new_harness_row(&self) -> bool {
        let rows = self.rail_rows();
        rows.get(self.agent_index.min(rows.len().saturating_sub(1)))
            .is_some_and(|row| row.is_new_harness())
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

    /// Handle a key while the rail holds focus.
    ///
    /// Returns [`AgentsKey::Unhandled`] for anything the rail has no opinion on
    /// — tab switching, transcript paging, the `Alt` steering chords — so those
    /// keep working identically from either side of the tab.
    pub(super) fn on_agents_rail_key(&mut self, k: KeyEvent) -> AgentsKey {
        if !self.agents_rail_focused() {
            return AgentsKey::Unhandled;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        match k.code {
            KeyCode::Char('K') => {
                if let Some(target) = self.watch_target() {
                    self.kill_armed = Some(target);
                    self.set_status("Kill this harness? y confirm · any other key cancels");
                } else {
                    self.set_status("Select a running harness task first");
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
            // the action row, where it is the action. A visible harness consumes
            // it earlier and takes the keyboard instead.
            KeyCode::Enter => {
                if self.on_new_harness_row() {
                    self.open_harness_picker();
                } else {
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
            // Typing lands in the composer and focus follows it — but only
            // where there *is* a composer. It is drawn for the orchestrator
            // lane and nowhere else, so doing this on an agent row moved the
            // keyboard into an invisible input: the character vanished, and
            // every arrow after it drove a caret nobody could see instead of
            // the rail. That reads exactly like navigation having died.
            KeyCode::Char(c) if !ctrl && !alt && self.on_orchestrator_lane() => {
                self.focus_agents_composer();
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
                self.command_index = 0;
                AgentsKey::Handled(None)
            }
            // On any other row the rail keeps the keyboard and says why, rather
            // than swallowing the key or stealing focus for a hidden box.
            KeyCode::Char(c) if !ctrl && !alt => {
                let _ = c;
                if self.on_new_harness_row() {
                    self.set_status("⏎ starts a harness of your own");
                } else {
                    self.set_status("Select the orchestrator lane to type an instruction");
                }
                AgentsKey::Handled(None)
            }
            // Backspace is typing too — it edits the draft it belongs to, and
            // only where that draft is on screen.
            KeyCode::Backspace if self.on_orchestrator_lane() => {
                self.focus_agents_composer();
                AgentsKey::Unhandled
            }
            _ => AgentsKey::Unhandled,
        }
    }
}
