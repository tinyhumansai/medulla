//! Appearance-setting mutations and persistence.
//!
//! Colour roles and local-process indicators live here. The two
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

/// Number of selectable rows on the Appearance page: theme roles plus resources.
pub(super) const APPEARANCE_ROWS: usize = THEME_ROLES.len() + 3;

impl App {
    /// Cycle the selected colour role and persist the theme.
    pub(super) fn cycle_appearance_row(&mut self, forward: bool) {
        let index = self.appearance_index.min(APPEARANCE_ROWS - 1);
        if index < THEME_ROLES.len() {
            self.theme.cycle_role(index, forward);
            self.persist_theme_now(THEME_ROLES[index]);
        } else {
            self.cycle_resource_display(index - THEME_ROLES.len(), forward);
        }
    }

    /// Cycle and persist one local-process resource indicator.
    fn cycle_resource_display(&mut self, index: usize, forward: bool) {
        use medulla::config::ResourceDisplay::{Bar, Off, Percent, Value};

        let (name, display, choices): (&str, &mut _, &[_]) = match index {
            0 => (
                "CPU",
                &mut self.loaded.config.appearance.cpu,
                &[Off, Percent, Bar],
            ),
            1 => (
                "RAM",
                &mut self.loaded.config.appearance.ram,
                &[Off, Percent, Value, Bar],
            ),
            _ => (
                "Disk I/O",
                &mut self.loaded.config.appearance.disk_io,
                &[Off, Value, Bar],
            ),
        };
        let current = choices
            .iter()
            .position(|choice| choice == display)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % choices.len()
        } else {
            (current + choices.len() - 1) % choices.len()
        };
        *display = choices[next];
        let value = format!("{:?}", *display).to_ascii_lowercase();

        match &self.config_path {
            Some(path) => {
                let mut section = toml::Table::new();
                section.insert(
                    "cpu".into(),
                    toml::Value::String(
                        format!("{:?}", self.loaded.config.appearance.cpu).to_ascii_lowercase(),
                    ),
                );
                section.insert(
                    "ram".into(),
                    toml::Value::String(
                        format!("{:?}", self.loaded.config.appearance.ram).to_ascii_lowercase(),
                    ),
                );
                section.insert(
                    "diskIo".into(),
                    toml::Value::String(
                        format!("{:?}", self.loaded.config.appearance.disk_io).to_ascii_lowercase(),
                    ),
                );
                section.insert(
                    "showHarnessBranch".into(),
                    toml::Value::Boolean(self.loaded.config.appearance.show_harness_branch),
                );
                section.insert(
                    "showHarnessPath".into(),
                    toml::Value::Boolean(self.loaded.config.appearance.show_harness_path),
                );
                match medulla::config::persist_section(path, "appearance", section) {
                    Ok(()) => self.set_status(format!("{name} indicator: {value} (saved)")),
                    Err(error) => self.set_status(format!("Appearance save failed: {error}")),
                }
            }
            None => self.set_status(format!("{name} indicator: {value} (not persisted)")),
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
}
