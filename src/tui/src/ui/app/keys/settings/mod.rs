//! Keyboard handling for the Settings tab and every subpage it hosts.
//!
//! Settings has two levels of focus, and the split is by *mode*: the left-hand
//! nav owns the keyboard until you step into the content pane with `Enter` (or
//! `→`), and `Esc` steps back out. While the nav has focus `↑↓` walk the subpage
//! list; once the pane has focus they browse its contents instead.
//!
//! An earlier design split by key rather than by mode — `↑↓` always drove the
//! nav, `j/k` and the subpage's letters drove the content. It avoided a focus
//! toggle, but it did not survive the letter-bound pages: they bind single letters
//! as actions, so the keys you would reach for to get around instead voted,
//! commented, or opened a submission, and the arrow keys jumped you off the page
//! entirely. Making entry explicit is what makes those letters deliberate.
//!
//! `j/k` still browse inside a focused pane, so the old muscle memory works.

use crossterm::event::KeyCode;

use crate::ui::multi_pane::{self, NavAction};
use crate::ui::theme::THEME_ROLES;

use super::super::types::{
    App, Cmd, SETTINGS_SUBPAGES, SP_ACCOUNT, SP_APPEARANCE, SP_CONFIG, SP_CONTEXT, SP_HELP,
    SP_TRACE, SP_USAGE,
};

impl App {
    /// Handle a key on the Settings tab.
    ///
    /// Returns `None` when the key is not one Settings claims, so the caller can
    /// fall through to the global bindings.
    pub(super) fn on_settings_key(&mut self, code: KeyCode) -> SettingsKey {
        let allow_leave = !self.logout_armed();
        match multi_pane::navigate(
            code,
            SETTINGS_SUBPAGES.len(),
            &mut self.settings_index,
            &mut self.settings_focused,
            allow_leave,
        ) {
            NavAction::SelectionChanged => {
                self.disarm_logout();
                return SettingsKey::handled(self.tab_enter_cmd());
            }
            NavAction::Entered => {
                self.set_status(format!(
                    "{} · Esc to go back to the menu",
                    self.settings_subpage()
                ));
                return SettingsKey::handled(None);
            }
            NavAction::Left => {
                self.set_status("Settings · menu");
                return SettingsKey::handled(None);
            }
            NavAction::Consumed => return SettingsKey::handled(None),
            NavAction::Unhandled => {}
        }

        // Content focus: arrows browse, everything else is the subpage's binding.
        match code {
            KeyCode::Up | KeyCode::Down => {
                let up = matches!(code, KeyCode::Up);
                return self.settings_content_scroll(up);
            }
            _ => {}
        }

        match self.settings_index {
            SP_USAGE => self.usage_key(code),
            SP_APPEARANCE => self.appearance_key(code),
            SP_CONFIG => self.config_key(code),
            SP_TRACE => self.trace_key(code),
            SP_CONTEXT => self.context_key(code),
            SP_ACCOUNT => self.account_key(code),
            _ => SettingsKey::Unhandled,
        }
    }

    /// Move the selection inside the focused subpage's content pane.
    ///
    /// This is what `↑↓` mean once the pane has focus; each subpage's `j/k`
    /// bindings stay as they were, so both work.
    fn settings_content_scroll(&mut self, up: bool) -> SettingsKey {
        match self.settings_index {
            SP_APPEARANCE => {
                self.appearance_index = if up {
                    self.appearance_index.saturating_sub(1)
                } else {
                    (self.appearance_index + 1).min(THEME_ROLES.len() - 1)
                };
                SettingsKey::handled(None)
            }
            SP_CONFIG => {
                self.move_config_index(up);
                SettingsKey::handled(None)
            }
            SP_TRACE => {
                self.selected = if up {
                    self.selected.saturating_sub(1)
                } else {
                    self.selected + 1
                };
                SettingsKey::handled(None)
            }
            SP_CONTEXT => {
                self.context_index = if up {
                    self.context_index.saturating_sub(1)
                } else {
                    (self.context_index + 1).min(self.contexts.len().saturating_sub(1))
                };
                SettingsKey::handled(None)
            }
            // Help is a single long page rather than a list, so it scrolls by
            // the line. It outgrew a short terminal once the harness bindings
            // and commands landed on it, and a reference you cannot reach the
            // bottom of is not one.
            SP_HELP => {
                self.help_scroll = if up {
                    self.help_scroll.saturating_sub(1)
                } else {
                    self.help_scroll.saturating_add(1)
                };
                SettingsKey::handled(None)
            }
            // Usage and Account have nothing to scroll; swallow the key so it
            // does not fall through to the global bindings and switch tabs.
            _ => SettingsKey::handled(None),
        }
    }

    /// Usage: refresh the account totals, or jump to the config editor.
    fn usage_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('r') => {
                self.set_status("Usage · refreshing…");
                SettingsKey::handled(Some(Cmd::LoadUsage))
            }
            KeyCode::Char('c') => SettingsKey::handled(self.set_settings_subpage(SP_CONFIG)),
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
                SettingsKey::handled(None)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                self.cycle_appearance_role(!matches!(code, KeyCode::Left));
                SettingsKey::handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Config: pick a setting and change it.
    fn config_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('j') | KeyCode::Char('k') => {
                self.move_config_index(matches!(code, KeyCode::Char('k')));
                SettingsKey::handled(None)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                let delta = match code {
                    KeyCode::Left => -1,
                    KeyCode::Right => 1,
                    _ => 0,
                };
                let status = self.adjust_setting(delta);
                self.set_status(status);
                SettingsKey::handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Trace: page through the event stream.
    fn trace_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                SettingsKey::handled(None)
            }
            KeyCode::Char('j') => {
                self.selected += 1;
                SettingsKey::handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Context: browse the environment chunks.
    fn context_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Char('k') => {
                self.context_index = self.context_index.saturating_sub(1);
                SettingsKey::handled(None)
            }
            KeyCode::Char('j') => {
                let max = self.contexts.len().saturating_sub(1);
                self.context_index = (self.context_index + 1).min(max);
                SettingsKey::handled(None)
            }
            KeyCode::Char('r') => {
                self.set_status("Context · refreshing…");
                SettingsKey::handled(Some(Cmd::InspectContext))
            }
            _ => SettingsKey::Unhandled,
        }
    }

    /// Account: arm and confirm the logout.
    fn account_key(&mut self, code: KeyCode) -> SettingsKey {
        match code {
            KeyCode::Enter => {
                let (status, cmd) = self.confirm_logout();
                self.set_status(status);
                SettingsKey::handled(cmd)
            }
            KeyCode::Esc => {
                self.disarm_logout();
                self.set_status("Account · logout cancelled");
                SettingsKey::handled(None)
            }
            _ => SettingsKey::Unhandled,
        }
    }
}

impl SettingsKey {
    /// Mark a key as consumed while keeping the comparatively large command
    /// payload out of the dispatch enum itself.
    fn handled(cmd: Option<Cmd>) -> Self {
        Self::Handled(Box::new(cmd))
    }
}

mod types;
pub(super) use types::SettingsKey;
