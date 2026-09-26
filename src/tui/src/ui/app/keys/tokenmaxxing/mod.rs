//! Keyboard handling for the TokenMaxxxing overview and bounty pages.

use crossterm::event::KeyCode;

use crate::ui::multi_pane::{self, NavAction};

use super::super::types::{App, Cmd, TOKENMAXXING_SUBPAGES};

impl App {
    /// Move between the TokenMaxxxing menu and its read-only content pages.
    pub(super) fn on_tokenmaxxing_key(&mut self, code: KeyCode) -> TokenMaxxxingKey {
        match multi_pane::navigate(
            code,
            TOKENMAXXING_SUBPAGES.len(),
            &mut self.tokenmaxxing_index,
            &mut self.tokenmaxxing_focused,
            true,
        ) {
            NavAction::SelectionChanged | NavAction::Consumed => TokenMaxxxingKey::Handled(None),
            NavAction::Entered => {
                self.set_status(format!(
                    "TokenMaxxxing · {} · Esc to go back to the menu",
                    TOKENMAXXING_SUBPAGES[self.tokenmaxxing_index]
                ));
                TokenMaxxxingKey::Handled(None)
            }
            NavAction::Left => {
                self.set_status("TokenMaxxxing · menu");
                TokenMaxxxingKey::Handled(None)
            }
            NavAction::Unhandled => TokenMaxxxingKey::Unhandled,
        }
    }
}

mod types;
pub(super) use types::TokenMaxxxingKey;
