//! Appearance-setting mutations and persistence.
//!
//! Color roles and harness-row details share the Appearance page, but they
//! persist to separate config sections: colors under `[theme]`, display choices
//! under `[appearance]`.

use crate::ui::theme::{color_to_string, THEME_ROLES};

use super::types::App;

/// Appearance row containing the harness branch toggle.
pub(super) const HARNESS_BRANCH_ROW: usize = THEME_ROLES.len();
/// Appearance row containing the shortened harness path toggle.
pub(super) const HARNESS_PATH_ROW: usize = THEME_ROLES.len() + 1;
/// Number of selectable rows on the Appearance page.
pub(super) const APPEARANCE_ROWS: usize = THEME_ROLES.len() + 2;

impl App {
    /// Activate the selected Appearance row.
    pub(super) fn cycle_appearance_row(&mut self, forward: bool) {
        match self.appearance_index.min(APPEARANCE_ROWS - 1) {
            index if index < THEME_ROLES.len() => {
                self.theme.cycle_role(index, forward);
                self.persist_theme_now(THEME_ROLES[index]);
            }
            HARNESS_BRANCH_ROW => {
                let enabled = !self.loaded.config.appearance.show_harness_branch;
                self.loaded.config.appearance.show_harness_branch = enabled;
                self.persist_harness_detail_now("showHarnessBranch", "branch", enabled);
            }
            HARNESS_PATH_ROW => {
                let enabled = !self.loaded.config.appearance.show_harness_path;
                self.loaded.config.appearance.show_harness_path = enabled;
                self.persist_harness_detail_now("showHarnessPath", "short path", enabled);
            }
            _ => unreachable!("appearance selection is clamped"),
        }
    }

    /// Write the current theme to the injected config path.
    fn persist_theme_now(&mut self, role: &str) {
        let value = color_to_string(self.theme.role(self.appearance_index));
        match &self.config_path {
            Some(path) => match crate::ui::theme::persist_theme(path, &self.theme) {
                Ok(()) => self.set_status(format!("Appearance · {role} → {value} (saved)")),
                Err(error) => self.set_status(format!("Appearance · save failed: {error}")),
            },
            None => self.set_status(format!("Appearance · {role} → {value} (not persisted)")),
        }
    }

    /// Persist one harness-row display choice without replacing other settings.
    fn persist_harness_detail_now(&mut self, key: &str, label: &str, enabled: bool) {
        let state = if enabled { "shown" } else { "hidden" };
        match &self.config_path {
            Some(path) => match medulla::config::persist_setting(
                path,
                "appearance",
                key,
                toml::Value::Boolean(enabled),
            ) {
                Ok(()) => self.set_status(format!("Appearance · harness {label} {state} (saved)")),
                Err(error) => self.set_status(format!("Appearance · save failed: {error}")),
            },
            None => self.set_status(format!(
                "Appearance · harness {label} {state} (not persisted)"
            )),
        }
    }
}
