//! Changes-tab prompt actions that mutate the in-memory review model.

use medulla::ui::git_review::CommentAnchor;

use super::super::changes::types::BaselineSource;
use super::super::types::{App, PromptKind};
use crate::ui::composer::{Draft, TextPrompt};

impl App {
    /// Open the baseline selector over the launch snapshot and recent history.
    pub(crate) fn open_change_baseline_picker(&mut self) {
        self.changes.picking_baseline = true;
        self.changes.baseline_index = 0;
        self.set_status("Baseline: ↑/↓ select · Enter apply · m manual · Esc cancel");
    }

    /// Apply the highlighted baseline option, opening manual input when needed.
    pub(crate) fn apply_change_baseline_selection(&mut self) {
        let index = self.changes.baseline_index;
        if index == 0 {
            match self.changes.choose_session_baseline() {
                Ok(()) => self.set_status(self.changes.status_message()),
                Err(error) => self.set_status(error),
            }
        } else if let Some(commit) = self.changes.recent_commits.get(index - 1) {
            self.finish_change_baseline(&commit.id.clone(), BaselineSource::Commit);
        } else {
            self.open_manual_change_baseline();
        }
    }

    /// Ask for an arbitrary commit id or revision.
    pub(crate) fn open_manual_change_baseline(&mut self) {
        self.changes.picking_baseline = false;
        self.prompt = Some(TextPrompt {
            title: "Diff from commit or revision".to_owned(),
            draft: Draft::new(),
            kind: PromptKind::ChangesBaseline,
        });
        self.set_status("Enter a commit id or revision · Enter apply · Esc cancel");
    }

    /// Validate and activate a requested baseline, preserving errors in status.
    pub(crate) fn finish_change_baseline(&mut self, revision: &str, source: BaselineSource) {
        match self.changes.choose_baseline(revision, source) {
            Ok(()) => self.set_status(self.changes.status_message()),
            Err(error) => self.set_status(error),
        }
    }

    /// Submit a Changes-tab comment while capturing context for drift detection.
    pub(super) fn submit_changes_comment(
        &mut self,
        path: &std::path::Path,
        anchor: CommentAnchor,
        text: &str,
    ) {
        let context = match anchor {
            CommentAnchor::Line(i) => self
                .changes
                .patch
                .get(i)
                .map(String::as_str)
                .unwrap_or("")
                .to_owned(),
            CommentAnchor::Hunk(i) => self
                .changes
                .hunks
                .get(i)
                .and_then(|h| self.changes.patch.get(h.header))
                .map(String::as_str)
                .unwrap_or("")
                .to_owned(),
            CommentAnchor::File => String::new(),
        };
        let kept = self
            .changes
            .comments
            .upsert_with_context(path, anchor, text, &context);
        self.set_status(if kept {
            format!(
                "Comment saved on {} · {}",
                path.display(),
                anchor.describe()
            )
        } else {
            format!("Comment cleared on {}", path.display())
        });
    }
}
