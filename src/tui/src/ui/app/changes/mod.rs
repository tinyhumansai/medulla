//! Session-scoped Git inspection and review comments for the Changes tab.

mod repository;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use types::GitChangesState;

use crate::ui::composer::{Draft, TextPrompt};

use super::types::{App, PromptKind};

impl App {
    /// Reload commits, changed paths, and the selected patch from Git.
    pub(super) fn refresh_changes(&mut self) {
        self.changes.refresh();
        self.set_status(self.changes.status_message());
    }

    /// Open a session-local comment prompt for the selected changed file.
    pub(super) fn comment_on_change(&mut self) {
        let Some(path) = self.changes.selected_path().map(str::to_owned) else {
            self.set_status("No changed file selected");
            return;
        };
        self.prompt = Some(TextPrompt {
            title: format!("Comment on {path}"),
            draft: Draft::new(),
            kind: PromptKind::ChangesComment { path },
        });
    }
}
