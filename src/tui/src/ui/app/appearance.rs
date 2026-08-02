//! Appearance-setting mutations and persistence.
//!
//! Only the colour roles live here now, persisted under `[theme]`. The two
//! harness-row toggles this page used to carry — branch and shortened path —
//! became placements on the Status line page, which is where the rest of the row
//! is configured and the only place the effect can be previewed. Leaving them
//! here as well would have meant two controls writing two sections for one
//! choice, with the newer one silently winning.
//!
//! The old `[appearance]` keys are still honoured for a config that predates
//! that move: see
//! [`StatusLineConfig::from_appearance`](medulla::config::StatusLineConfig::from_appearance).

use crate::ui::theme::{color_to_string, THEME_ROLES};

use super::types::App;

/// Number of selectable rows on the Appearance page — one per theme role.
pub(super) const APPEARANCE_ROWS: usize = THEME_ROLES.len();

impl App {
    /// Cycle the selected colour role and persist the theme.
    pub(super) fn cycle_appearance_row(&mut self, forward: bool) {
        let index = self.appearance_index.min(APPEARANCE_ROWS - 1);
        self.theme.cycle_role(index, forward);
        self.persist_theme_now(THEME_ROLES[index]);
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
}
