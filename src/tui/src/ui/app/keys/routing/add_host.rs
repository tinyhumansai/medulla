//! Keyboard handling for the Add Host wizard.
//!
//! Split from the Routing key module because it is a flow with its own keys
//! rather than the single-key actions its sibling panes use, and `mod.rs` is for
//! wiring.

use crossterm::event::KeyCode;

use medulla::daemon::pairing::REMOTE_JOIN_COMMAND;

use super::super::super::types::App;
use super::RoutingKey;

impl App {
    /// Open the host-address prompt, or copy the line to run on the machine
    /// being added.
    ///
    /// There is no kind to choose: this page adds another *machine*. A harness
    /// plus a directory on this one is an agent, and it is declared from the
    /// host tree (`n`) rather than here.
    pub(super) fn add_host_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Enter | KeyCode::Char('a') => {
                self.open_add_host_prompt();
                RoutingKey::Handled(None)
            }
            // Copying it here rather than retyping it there is the whole point:
            // this end is a local terminal, so the copy is free.
            KeyCode::Char('c') => {
                self.copy_line("the worker install line", REMOTE_JOIN_COMMAND);
                RoutingKey::Handled(None)
            }
            // The page is instructions — there is no list to walk. Consumed so
            // an arrow cannot fall through to the pane navigation and move the
            // operator off a page they are reading.
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                RoutingKey::Handled(None)
            }
            _ => RoutingKey::Unhandled,
        }
    }
}
