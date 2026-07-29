//! Editing and persistence for OpenRouter-backed coding harness presets.
//!
//! The editor stores only the environment-variable name that holds the key.
//! Secret values stay in the process environment and never enter app state or
//! the configuration document.

use super::types::{App, Prompt, PromptKind};

const EDITOR_FORMAT: &str = "id | name | claude|codex | model | fast-model | host-id";

impl App {
    /// Move the custom-harness cursor one row.
    pub(super) fn move_custom_harness_selection(&mut self, up: bool) {
        self.custom_harness_index =
            crate::ui::selection::moved(self.custom_harness_index, self.custom_harnesses.len(), up);
    }

    /// Return the selected custom harness, if the list is non-empty.
    pub(super) fn selected_custom_harness(&self) -> Option<&medulla::config::CustomHarnessConfig> {
        self.custom_harnesses.get(self.custom_harness_index)
    }

    /// Open an editor prefilled for a useful Claude Code + OpenRouter preset.
    pub(super) fn open_add_custom_harness(&mut self) {
        let host_id = self.loaded.config.host.address.trim();
        let host_id = if host_id.is_empty() {
            "this-device"
        } else {
            host_id
        };
        self.prompt = Some(Prompt::with_text(
            PromptKind::CustomHarnessAdd,
            format!("Add custom harness — {EDITOR_FORMAT}"),
            format!(
                "deepseek | DeepSeek via Claude | claude | deepseek/deepseek-chat | | {host_id}"
            ),
        ));
        self.set_status("Custom harness · Enter save · Esc cancel");
    }

    /// Open the compact editor for the selected preset.
    pub(super) fn open_edit_custom_harness(&mut self) {
        let Some(harness) = self.selected_custom_harness().cloned() else {
            self.set_status("Add a custom harness first");
            return;
        };
        self.prompt = Some(Prompt::with_text(
            PromptKind::CustomHarnessEdit(harness.id.clone()),
            format!("Edit custom harness — {EDITOR_FORMAT}"),
            harness.editor_line(),
        ));
        self.set_status("Custom harness · Enter save · Esc cancel");
    }

    /// Parse and persist an added or edited custom harness.
    pub(super) fn save_custom_harness(&mut self, old_id: Option<&str>, text: &str) {
        let mut harness = match medulla::config::CustomHarnessConfig::from_editor_line(text) {
            Ok(harness) => harness,
            Err(error) => {
                self.set_status(format!("Custom harness · {error}"));
                return;
            }
        };

        if let Some(old_id) = old_id {
            let Some(index) = self
                .custom_harnesses
                .iter()
                .position(|candidate| candidate.id == old_id)
            else {
                self.set_status("Custom harness no longer exists");
                return;
            };
            if self
                .custom_harnesses
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.id == harness.id)
            {
                self.set_status(format!("Custom harness id '{}' already exists", harness.id));
                return;
            }
            let previous = &self.custom_harnesses[index];
            harness.default = previous.default;
            harness.api_key_env = previous.api_key_env.clone();
            harness.base_url = previous.base_url.clone();
            harness.context_window = previous.context_window;
            self.custom_harnesses[index] = harness;
            self.custom_harness_index = index;
        } else {
            if self
                .custom_harnesses
                .iter()
                .any(|candidate| candidate.id == harness.id)
            {
                self.set_status(format!("Custom harness id '{}' already exists", harness.id));
                return;
            }
            self.custom_harnesses.push(harness);
            self.custom_harness_index = self.custom_harnesses.len() - 1;
        }
        self.persist_custom_harnesses("Custom harness saved");
    }

    /// Remove and persist the selected preset.
    pub(super) fn delete_selected_custom_harness(&mut self) {
        if self.custom_harnesses.is_empty() {
            self.set_status("No custom harness to delete");
            return;
        }
        let removed = self.custom_harnesses.remove(self.custom_harness_index);
        self.custom_harness_index =
            crate::ui::selection::clamp(self.custom_harness_index, self.custom_harnesses.len());
        self.persist_custom_harnesses(&format!("Removed custom harness '{}'", removed.name));
    }

    /// Re-read presets from the active config file.
    pub(super) fn reload_custom_harnesses(&mut self) {
        let Some(path) = &self.config_path else {
            return;
        };
        match medulla::config::load_custom_harnesses(path) {
            Ok(harnesses) => {
                self.custom_harnesses = harnesses;
                self.custom_harness_index = crate::ui::selection::clamp(
                    self.custom_harness_index,
                    self.custom_harnesses.len(),
                );
            }
            Err(error) => self.set_status(format!("Cannot load custom harnesses: {error}")),
        }
    }

    /// Write the in-memory preset list without exposing any key values.
    fn persist_custom_harnesses(&mut self, success: &str) {
        let Some(path) = &self.config_path else {
            self.set_status("Custom harness changed for this session; no config path is writable");
            return;
        };
        match medulla::config::persist_custom_harnesses(path, &self.custom_harnesses) {
            Ok(()) => self.set_status(format!("{success} · restart host to apply")),
            Err(error) => self.set_status(format!("Cannot save custom harnesses: {error}")),
        }
    }
}

#[cfg(test)]
mod tests;
