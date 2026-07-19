//! Feedback-tab behaviour: selection, the query controls (sort/filter/refresh),
//! and the vote/comment/submit actions.
//!
//! Every mutating action returns the [`Cmd`] the event loop runs; the results
//! come back as `AppMsg`s and land through the setters here. Board state is
//! server-owned, so the app holds only the current page and the selected item's
//! comments.

use medulla::client::{
    FeedbackComment, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSort, FeedbackType,
};

use crate::ui::composer::Draft;

use super::types::{App, Cmd, Prompt, PromptKind};

impl App {
    /// The highlighted board item, if any.
    pub(super) fn feedback_selected(&self) -> Option<&FeedbackItem> {
        self.feedback.items.get(self.feedback.index)
    }

    /// The active board query, for re-issuing a load after a mutation.
    pub fn feedback_query(&self) -> FeedbackQuery {
        self.feedback.query.clone()
    }

    /// The command that loads the selected item's comments, unless they are
    /// already loaded.
    pub fn feedback_detail_cmd(&self) -> Option<Cmd> {
        let item = self.feedback_selected()?;
        if self.feedback.detail_id.as_deref() == Some(item.id.as_str()) {
            return None;
        }
        Some(Cmd::LoadFeedbackDetail(item.id.clone()))
    }

    /// Move the selection and load the newly selected item's comments.
    pub(super) fn move_feedback_index(&mut self, up: bool) -> Option<Cmd> {
        let max = self.feedback.items.len().saturating_sub(1);
        self.feedback.index = if up {
            self.feedback.index.saturating_sub(1)
        } else {
            (self.feedback.index + 1).min(max)
        };
        self.feedback.detail_scroll = 0;
        self.feedback_detail_cmd()
    }

    /// Reload the board with the current query.
    pub(super) fn reload_feedback(&mut self) -> Option<Cmd> {
        self.feedback.loading = true;
        Some(Cmd::LoadFeedback(self.feedback.query.clone()))
    }

    /// Cycle the board ordering (hot → top → new) and reload.
    pub(super) fn cycle_feedback_sort(&mut self) -> Option<Cmd> {
        self.feedback.query.sort = self.feedback.query.sort.next();
        self.feedback.query.page = 1;
        self.set_status(format!(
            "Feedback · sorting by {}",
            self.feedback.query.sort.as_str()
        ));
        self.reload_feedback()
    }

    /// Cycle the type filter (all → features → bugs) and reload.
    pub(super) fn cycle_feedback_filter(&mut self) -> Option<Cmd> {
        self.feedback.query.kind = match self.feedback.query.kind {
            None => Some(FeedbackType::Feature),
            Some(FeedbackType::Feature) => Some(FeedbackType::Bug),
            _ => None,
        };
        self.feedback.query.page = 1;
        let label = match self.feedback.query.kind {
            None => "everything",
            Some(FeedbackType::Bug) => "bugs",
            _ => "feature requests",
        };
        self.set_status(format!("Feedback · showing {label}"));
        self.reload_feedback()
    }

    /// Vote on the selected item. Re-pressing the key for a vote already cast
    /// retracts it, so `u` and `d` act as toggles.
    pub(super) fn vote_selected_feedback(&mut self, value: i8) -> Option<Cmd> {
        let item = self.feedback_selected()?;
        let effective = if item.my_vote == value { 0 } else { value };
        let id = item.id.clone();
        self.set_status(match effective {
            1 => "Feedback · upvoting…",
            -1 => "Feedback · downvoting…",
            _ => "Feedback · retracting vote…",
        });
        Some(Cmd::VoteFeedback {
            id,
            value: effective,
        })
    }

    /// Open the inline prompt for commenting on the selected item.
    pub(super) fn open_feedback_comment(&mut self) {
        let Some(item) = self.feedback_selected() else {
            self.set_status("Select an item to comment on.");
            return;
        };
        let title = format!("Comment on “{}”", crate::ui::util::clip(&item.title, 40));
        self.prompt = Some(Prompt {
            kind: PromptKind::FeedbackComment {
                id: item.id.clone(),
            },
            title,
            draft: Draft::new(),
        });
        self.set_status("Comment · Enter post · Esc cancel");
    }

    /// Open step one (the title) of submitting new feedback.
    pub(super) fn open_feedback_submit(&mut self, kind: FeedbackType) {
        let what = match kind {
            FeedbackType::Bug => "bug report",
            _ => "feature request",
        };
        self.prompt = Some(Prompt {
            kind: PromptKind::FeedbackTitle { kind },
            title: format!("New {what} — title"),
            draft: Draft::new(),
        });
        self.set_status("New feedback · Enter next · Esc cancel");
    }

    /// Advance from the title prompt to the body prompt.
    pub(super) fn open_feedback_body(&mut self, kind: FeedbackType, title: String) {
        self.prompt = Some(Prompt {
            kind: PromptKind::FeedbackBody {
                kind,
                title: title.clone(),
            },
            title: format!("Describe it — “{}”", crate::ui::util::clip(&title, 34)),
            draft: Draft::new(),
        });
        self.set_status("New feedback · Enter submit · Esc cancel");
    }

    // --- setters, driven by the event loop --------------------------------

    /// Store a freshly loaded board page. `None` means this runtime has no
    /// board, which the tab renders as a sign-in hint.
    pub fn set_feedback_page(&mut self, page: Option<FeedbackPage>) {
        self.feedback.loading = false;
        match page {
            None => {
                self.feedback.supported = false;
                self.feedback.items.clear();
                self.feedback.total = 0;
            }
            Some(page) => {
                self.feedback.supported = true;
                self.feedback.total = page.total;
                self.feedback.items = page.items;
                self.feedback.index = self
                    .feedback
                    .index
                    .min(self.feedback.items.len().saturating_sub(1));
                // The previously loaded comments may belong to an item that is
                // no longer on this page.
                if !self
                    .feedback
                    .items
                    .iter()
                    .any(|i| Some(i.id.as_str()) == self.feedback.detail_id.as_deref())
                {
                    self.feedback.detail_id = None;
                    self.feedback.comments.clear();
                }
            }
        }
    }

    /// Store the comments loaded for `id`.
    pub fn set_feedback_comments(&mut self, id: String, comments: Vec<FeedbackComment>) {
        self.feedback.detail_id = Some(id);
        self.feedback.comments = comments;
        self.feedback.detail_scroll = 0;
    }

    /// Replace an item in the loaded page with the server's updated copy (after
    /// a vote). Leaves the selection where it is.
    pub fn apply_feedback_item(&mut self, item: FeedbackItem) {
        if let Some(slot) = self.feedback.items.iter_mut().find(|i| i.id == item.id) {
            *slot = item;
        }
    }

    /// The active feedback selection index. Test/inspection seam.
    pub fn feedback_index(&self) -> usize {
        self.feedback.index
    }

    /// The loaded board rows. Test/inspection seam.
    pub fn feedback_items(&self) -> &[FeedbackItem] {
        &self.feedback.items
    }

    /// The current sort, as its wire label. Test/inspection seam.
    pub fn feedback_sort(&self) -> &'static str {
        self.feedback.query.sort.as_str()
    }
}

/// The human label for a sort, used in the tab header.
pub(super) fn sort_label(sort: FeedbackSort) -> &'static str {
    match sort {
        FeedbackSort::Hot => "hot",
        FeedbackSort::Top => "top",
        FeedbackSort::New => "new",
    }
}
