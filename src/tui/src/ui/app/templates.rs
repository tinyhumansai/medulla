//! The Agent Templates page's state: where this client's catalog is installed
//! from, and how an installed catalog is re-read.
//!
//! The catalog itself is the SDK's ([`medulla::agents`]); this is the app-side
//! half — which directory the operator's `i` writes into, and how the result
//! reaches the running config so the page reflects the files immediately
//! instead of after a restart.

use std::path::PathBuf;

use super::types::App;

impl App {
    /// The directory this client's template store lives in.
    ///
    /// The injected home when a caller set one (tests, and `main` once it has
    /// resolved the real one), otherwise the env-derived home. Never the
    /// project-local store: installing writes the catalog once, for the user,
    /// rather than scattering a copy into every repository they open.
    pub(in crate::ui::app) fn template_store_dir(&self) -> PathBuf {
        match &self.medulla_home {
            Some(home) => home.join("agents"),
            None => {
                let env: std::collections::HashMap<String, String> = std::env::vars().collect();
                medulla::home::medulla_home(&env).join("agents")
            }
        }
    }

    /// How many templates the catalog page is listing. Test/inspection seam.
    pub fn template_row_count(&self) -> usize {
        crate::ui::fleet::template_rows(&self.fleet_capacity(), &self.fleet_roster()).len()
    }

    /// Write the built-in catalog into the store, then re-read it.
    ///
    /// Existing files are left alone, so this is safe to run on a store the
    /// operator has already edited: it fills in roles that are missing and
    /// reports how many it left as they were. The status line carries the
    /// outcome, including the directory, because "installed" without a path
    /// leaves the operator hunting for the files they are meant to edit.
    pub fn install_default_templates(&mut self) {
        let dir = self.template_store_dir();
        match medulla::agents::install_default_templates(&dir) {
            Ok(outcome) => {
                let summary = outcome.summary();
                self.reload_templates();
                self.set_status(summary);
            }
            Err(err) => self.set_status(format!(
                "Cannot install templates into {}: {err}",
                dir.display()
            )),
        }
    }

    /// Re-read the template store into the running config.
    ///
    /// A store with nothing in it leaves the config alone — the built-in
    /// catalog, or whatever the config file declared, is still the better answer
    /// than an empty page. Unreadable files are surfaced on the status line;
    /// they cost only themselves.
    pub fn reload_templates(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_dir = self.template_store_dir();
        let project_dir = cwd.join(".medulla").join("agents");
        // Under `MEDULLA_DEV=1` the home *is* `./.medulla`, so the two are one
        // directory; reading it twice would make every template shadow itself.
        let mut dirs = vec![home_dir.clone()];
        if project_dir != home_dir {
            dirs.push(project_dir);
        }
        let store = medulla::agents::load_templates(&dirs);
        if !store.is_empty() {
            self.loaded.config.fleet.agent_templates = store.templates;
        }
        if let Some(first) = store.errors.first() {
            self.set_status(format!("Template store: {first}"));
        }
    }
}
