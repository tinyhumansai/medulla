//! Keyboard handling for the Settings tab and every subpage it hosts.
//!
//! Settings has two levels of focus, and the split is by key rather than by
//! mode: `↑↓` always move the left-nav between subpages, while `j/k` and the
//! subpage's own letter keys act inside the content pane. That keeps the nav
//! reachable from any subpage without a focus-toggle, and it is why Feedback —
//! which used `↑↓` and `j/k` interchangeably as a tab — now browses with `j/k`.

use crossterm::event::KeyCode;

use crate::ui::theme::THEME_ROLES;
use medulla::client::FeedbackType;

use super::super::types::{
    App, Cmd, SETTINGS_SUBPAGES, SP_ACCOUNT, SP_APPEARANCE, SP_CONFIG, SP_CONTEXT, SP_FEEDBACK,
    SP_TRACE, SP_USAGE,
};

impl App {
    /// Handle a key on the Settings tab.
    ///
    /// Returns `None` when the key is not one Settings claims, so the caller can
    /// fall through to the global bindings.
    pub(super) fn on_settings_key(&mut self, code: KeyCode) -> SettingsKey {
        // Digit jumps and nav moves apply on every subpage.
        match code {
            KeyCode::Char(d @ '1'..='9') => {
                let index = d as usize - '1' as usize;
                if index < SETTINGS_SUBPAGES.len() {
                    return SettingsKey::Handled(self.set_settings_subpage(index));
                }
                return SettingsKey::Unhandled;
            }
            KeyCode::Up | KeyCode::Down => {
                let up = matches!(code, KeyCode::Up);
                self.disarm_logout();
                self.settings_index = if up {
                    self.settings_index.saturating_sub(1)
                } else {
                    (self.settings_index + 1).min(SETTINGS_SUBPAGES.len() - 1)
                };
                return SettingsKey::Handled(self.tab_enter_cmd());
            }
            _ => {}
        }

        match self.settings_index {
            SP_USAGE => self.usage_key(code),
            SP_APPEARANCE => self.appearance_key(code),
            SP_CONFIG => self.config_key(code),
            SP_FEEDBACK => self.feedback_key(code),
            SP_TRACE => self.trace_key(code),
            SP_CONTEXT => self.context_key(code),
            SP_ACCOUNT => self.account_key(code),
            _ => SettingsKey::Unhandled,
        }
    }

    /// Usage: refresh the account totals, or jump to the config editor.
    fn usage_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('r') => {
                self.set_status("Usage · refreshing…");
                SettingsKey::Handled(Some(Cmd::LoadUsage))
            }
            KeyCode::Char('c') => SettingsKey::Handled(self.set_settings_subpage(SP_CONFIG)),
            _ => SettingsKey::Unhandled,
        }
    }

    /// Appearance: pick a theme role and cycle its color.
    fn appearance_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('j') | KeyCode::Char('k') => {
                let up = matches!(code, KeyCode::Char('k'));
                self.appearance_index = if up {
                    self.appearance_index.saturating_sub(1)
                } else {
                    (self.appearance_index + 1).min(THEME_ROLES.len() - 1)
                };
                SettingsKey::Handled(None)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                self.cycle_appearance_role(!matches!(code, KeyCode::Left));
                SettingsKey::Handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Config: pick a setting and change it.
    fn config_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('j') | KeyCode::Char('k') => {
                self.move_config_index(matches!(code, KeyCode::Char('k')));
                SettingsKey::Handled(None)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                let delta = match code {
                    KeyCode::Left => -1,
                    KeyCode::Right => 1,
                    _ => 0,
                };
                let status = self.adjust_setting(delta);
                self.set_status(status);
                SettingsKey::Handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Feedback: browse the board, vote, comment, and submit.
    fn feedback_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('k') => SettingsKey::Handled(self.move_feedback_index(true)),
            KeyCode::Char('j') => SettingsKey::Handled(self.move_feedback_index(false)),
            KeyCode::Char('u') => SettingsKey::Handled(self.vote_selected_feedback(1)),
            KeyCode::Char('d') => SettingsKey::Handled(self.vote_selected_feedback(-1)),
            KeyCode::Char('c') => {
                self.open_feedback_comment();
                SettingsKey::Handled(None)
            }
            KeyCode::Char('n') => {
                self.open_feedback_submit(FeedbackType::Feature);
                SettingsKey::Handled(None)
            }
            KeyCode::Char('b') => {
                self.open_feedback_submit(FeedbackType::Bug);
                SettingsKey::Handled(None)
            }
            KeyCode::Char('s') => SettingsKey::Handled(self.cycle_feedback_sort()),
            KeyCode::Char('f') => SettingsKey::Handled(self.cycle_feedback_filter()),
            KeyCode::Char('r') | KeyCode::Enter => {
                self.set_status("Feedback · refreshing…");
                SettingsKey::Handled(self.reload_feedback())
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Trace: page through the event stream.
    fn trace_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                SettingsKey::Handled(None)
            }
            KeyCode::Char('j') => {
                self.selected += 1;
                SettingsKey::Handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Context: browse the environment chunks.
    fn context_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('k') => {
                self.context_index = self.context_index.saturating_sub(1);
                SettingsKey::Handled(None)
            }
            KeyCode::Char('j') => {
                let max = self.contexts.len().saturating_sub(1);
                self.context_index = (self.context_index + 1).min(max);
                SettingsKey::Handled(None)
            }
            KeyCode::Char('r') => {
                self.set_status("Context · refreshing…");
                SettingsKey::Handled(Some(Cmd::InspectContext))
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Account: arm and confirm the logout.
    fn account_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Enter => {
                let status = self.confirm_logout();
                self.set_status(status);
                SettingsKey::Handled(None)
            }
            KeyCode::Esc => {
                self.disarm_logout();
                self.set_status("Account · logout cancelled");
                SettingsKey::Handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }
}

/// Whether the Settings dispatcher consumed a key, and any command it produced.
pub(super) enum SettingsKey {
    /// Settings handled the key; run the enclosed command, if any.
    Handled(Option<Cmd>),
    /// Settings does not bind this key — fall through to the global bindings.
    Unhandled,
}
