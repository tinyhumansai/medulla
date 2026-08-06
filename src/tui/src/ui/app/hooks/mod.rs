//! Editing and persistence for the Hooks page.
//!
//! Medulla is the one place hooks are declared for every harness it launches:
//! an operator writes a hook once here, and it is installed into Claude Code,
//! Codex, or whatever else the spawn resolves to, in that harness's own config
//! language (see [`medulla::harness_hooks`]). The alternative — the same policy
//! written three times, in three files, drifting the moment one is edited — is
//! what this page exists to replace.
//!
//! Two kinds of row share the list and must never be confused:
//!
//! - **Medulla's own** reporting hooks ([`medulla::harness_hooks::builtin`]),
//!   resolved at config load. They are shown, and they can be switched off as a
//!   set, but they are not editable and are never written to the operator's
//!   config file.
//! - **The operator's**, from `[[hooks]]`. These are what `a`, `e`, and `d`
//!   change, and the only ones that reach disk.

use medulla::harness_hooks::{HookSpec, HooksConfig};

use super::types::{App, Prompt, PromptKind};

#[cfg(test)]
mod tests;

/// The one-line editor form, quoted in every prompt title so the format is
/// where the typing happens rather than in documentation.
const EDITOR_FORMAT: &str = "Event | matcher | harnesses | timeout | command";

impl App {
    /// Every hook this Medulla would install, built-ins first.
    ///
    /// Read from the loaded config rather than a second copy: config load is
    /// where built-ins are resolved, so this is exactly what a harness launched
    /// right now would carry.
    pub(super) fn hook_rows(&self) -> &[HookSpec] {
        &self.loaded.config.hooks.hooks
    }

    /// The selected hook, if there is one.
    pub(super) fn selected_hook(&self) -> Option<&HookSpec> {
        self.hook_rows().get(self.hook_index)
    }

    /// Move the hook cursor one row.
    pub(super) fn move_hook_selection(&mut self, up: bool) {
        self.hook_index = crate::ui::selection::moved(self.hook_index, self.hook_rows().len(), up);
    }

    /// Open an editor prefilled with a hook worth starting from.
    pub(super) fn open_add_hook(&mut self) {
        self.prompt = Some(Prompt::with_text(
            PromptKind::HookAdd,
            format!("Add hook — {EDITOR_FORMAT}"),
            "PostToolUse | Edit,Write |  |  | ".to_string(),
        ));
        self.set_status("Hook · Enter save · Esc cancel");
    }

    /// Open the editor for the selected hook.
    ///
    /// Refuses on a built-in rather than opening an editor whose save would be
    /// discarded: Medulla's own hooks are replaced wholesale at every load, so
    /// an edit here could not survive one.
    pub(super) fn open_edit_hook(&mut self) {
        let Some(hook) = self.selected_hook().cloned() else {
            self.set_status("Add a hook first");
            return;
        };
        if hook.builtin {
            self.set_status("Medulla's own hooks cannot be edited — press b to turn them off");
            return;
        }
        let index = self.hook_index;
        self.prompt = Some(Prompt::with_text(
            PromptKind::HookEdit(index),
            format!("Edit hook — {EDITOR_FORMAT}"),
            hook.editor_line(),
        ));
        self.set_status("Hook · Enter save · Esc cancel");
    }

    /// Parse and persist an added or edited hook.
    ///
    /// `at` is the row being replaced, or `None` to append. A row that is no
    /// longer where it was — the config was reloaded under the editor — is
    /// reported rather than overwriting whatever now sits there.
    pub(super) fn save_hook(&mut self, at: Option<usize>, text: &str) {
        let mut hook = match HookSpec::from_editor_line(text) {
            Ok(hook) => hook,
            Err(error) => {
                self.set_status(format!("Hook · {error}"));
                return;
            }
        };
        let mut hooks = self.loaded.config.hooks.hooks.clone();
        match at {
            Some(index) if index < hooks.len() && !hooks[index].builtin => {
                // The editor form has no label field, so parsing one always
                // yields `None` — carry the row's existing label forward
                // rather than dropping it on every edit.
                hook.label = hooks[index].label.clone();
                hooks[index] = hook;
                self.hook_index = index;
            }
            Some(_) => {
                self.set_status("That hook is no longer there — reopen the page and retry");
                return;
            }
            None => {
                hooks.push(hook);
                self.hook_index = hooks.len() - 1;
            }
        }
        self.persist_hooks(HooksConfig { hooks }, "Hook saved");
    }

    /// Remove the selected hook.
    pub(super) fn delete_selected_hook(&mut self) {
        let Some(hook) = self.selected_hook().cloned() else {
            self.set_status("No hook to delete");
            return;
        };
        if hook.builtin {
            self.set_status("Medulla's own hooks cannot be deleted — press b to turn them off");
            return;
        }
        let mut hooks = self.loaded.config.hooks.hooks.clone();
        hooks.remove(self.hook_index);
        self.hook_index = crate::ui::selection::clamp(self.hook_index, hooks.len());
        let removed = hook.event.as_str().to_string();
        self.persist_hooks(
            HooksConfig { hooks },
            &format!("Removed the {removed} hook"),
        );
    }

    /// Turn Medulla's own reporting hooks on or off.
    ///
    /// Written to config *and* applied in memory, so the next harness the
    /// operator opens carries the answer they just gave rather than the one the
    /// process started with.
    pub(super) fn toggle_builtin_hooks(&mut self) {
        let enabled = !self.loaded.config.hook_defaults.enabled;
        // `[hookDefaults] enabled` is a plain switch, not the operator's own
        // `[[hooks]]` list — `load_config` only strips the latter from a
        // project-local layer — so the general `config_path` is the right
        // target here, unlike `persist_hooks` below.
        let Some(path) = &self.config_path else {
            self.set_status("No config path is writable — Medulla's hooks are unchanged");
            return;
        };
        if let Err(error) = medulla::config::persist_hook_defaults(path, enabled) {
            self.set_status(format!("Cannot save: {error}"));
            return;
        }
        self.loaded.config.hook_defaults.enabled = enabled;
        let operator = HooksConfig {
            hooks: self.loaded.config.hooks.operator_hooks(),
        };
        self.loaded.config.hooks = if enabled {
            operator.with_builtin(medulla::harness_hooks::builtin::hooks(
                &medulla::harness_hooks::builtin::medulla_bin(),
            ))
        } else {
            operator
        };
        self.sync_local_sessions_hooks();
        self.hook_index = crate::ui::selection::clamp(self.hook_index, self.hook_rows().len());
        self.set_status(if enabled {
            "Medulla's own hooks on · new harnesses report their lifecycle"
        } else {
            "Medulla's own hooks off · new harnesses report nothing"
        });
    }

    /// Write the operator's hooks and adopt `hooks` for this session.
    ///
    /// Targets [`super::types::App::hooks_config_path`], not the general
    /// `config_path`: a project-local config (`.medulla/config.toml` /
    /// `medulla.toml`) is exactly the layer `load_config` strips `[[hooks]]`
    /// from on the next non-explicit load, so writing there would show "Hook
    /// saved" while leaving a file the running Medulla ignores after restart
    /// (chatgpt-codex-connector P2 on PR #192).
    fn persist_hooks(&mut self, hooks: HooksConfig, success: &str) {
        let Some(path) = self.hooks_config_path.clone() else {
            self.set_status("Hook changed for this session; no config path is writable");
            self.loaded.config.hooks = hooks;
            self.sync_local_sessions_hooks();
            return;
        };
        // Only the operator's own: writing Medulla's built-ins into their file
        // would pin one release's defaults there forever.
        match medulla::config::persist_hooks(&path, &hooks.operator_hooks()) {
            Ok(()) => {
                self.loaded.config.hooks = hooks;
                self.sync_local_sessions_hooks();
                // Tell the operator when this landed somewhere other than the
                // path other settings save to, so a hook that "saved" but is
                // not in the project's own config is not a mystery.
                let note = if self.config_path.as_deref() == Some(path.as_path()) {
                    format!("{success} · applies to harnesses started from now on")
                } else {
                    format!(
                        "{success} in {} · project config cannot authorize hooks",
                        path.display()
                    )
                };
                self.set_status(note)
            }
            Err(error) => self.set_status(format!("Cannot save hooks: {error}")),
        }
    }

    /// Carry the just-saved hook set into the live operator-started launch
    /// path, not only `self.loaded.config.hooks`.
    ///
    /// `LocalSessions` (the seam `open_unmanaged` reads — see
    /// `harness_pane::spawn`) is handed its own clone of the resolved hooks
    /// once at startup, so an edit here previously updated the config the
    /// *next restart* would read while a session opened from the Agents pane
    /// before that restart kept launching with the hook set from before the
    /// edit: a hook just added would not run, and one just removed still
    /// would, even though the status line claimed "applies to harnesses
    /// started from now on" (chatgpt-codex-connector P2 on PR #192). `None`
    /// when this device is not hosting — there is then no live launch state
    /// to carry the edit into.
    fn sync_local_sessions_hooks(&mut self) {
        if let Some(sessions) = self.local_sessions.as_mut() {
            sessions.hooks = self.loaded.config.hooks.clone();
        }
    }
}
