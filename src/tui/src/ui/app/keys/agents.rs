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
//! anywhere returns to the composer with the character intact. This mirrors the
//! menu/content model [`multi_pane`](crate::ui::multi_pane) already gives
//! Settings and Routing. `Alt`+`↑`/`↓` still works for anyone whose terminal
//! sends it.

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
    /// Whether the Agents rail currently holds the keyboard.
    pub fn agents_rail_focused(&self) -> bool {
        self.agents_focus == AgentsFocus::Rail
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
            // The bare arrows are the point of having focus at all.
            KeyCode::Up | KeyCode::Down => {
                self.agent_scroll = 0;
                self.chat_scroll = 0;
                self.move_agent_index(matches!(k.code, KeyCode::Up));
                AgentsKey::Handled(None)
            }
            // A template row has no transcript, so Enter opens its declaration
            // rather than dropping focus into a composer that would submit to
            // the orchestrator instead — the same rule the composer applies.
            KeyCode::Enter
                if self
                    .selected_fleet_node()
                    .is_some_and(|node| node.key.starts_with("template:")) =>
            {
                self.template_modal = !self.template_modal;
                self.template_scroll = 0;
                AgentsKey::Handled(None)
            }
            // Enter is "I have found the row I wanted; let me type".
            KeyCode::Enter => {
                self.focus_agents_composer();
                AgentsKey::Handled(None)
            }
            // Esc closes the popup if one is up, else returns to the composer,
            // so the key that moved focus out also brings it back.
            KeyCode::Esc => {
                if self.template_modal {
                    self.template_modal = false;
                    self.template_scroll = 0;
                } else {
                    self.focus_agents_composer();
                }
                AgentsKey::Handled(None)
            }
            // Typing is never swallowed: the character lands in the composer and
            // focus follows it. Without this, a user who stepped out to look at
            // a lane would type a whole message into nothing.
            KeyCode::Char(c) if !ctrl && !alt => {
                self.focus_agents_composer();
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
                self.command_index = 0;
                AgentsKey::Handled(None)
            }
            // Backspace is typing too — it edits the draft it belongs to.
            KeyCode::Backspace => {
                self.focus_agents_composer();
                AgentsKey::Unhandled
            }
            _ => AgentsKey::Unhandled,
        }
    }
}
