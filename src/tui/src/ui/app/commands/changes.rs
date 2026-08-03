//! Changes-tab prompt actions that mutate the in-memory review model.

use medulla::ui::git_review::CommentAnchor;

use super::super::types::App;

impl App {
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
