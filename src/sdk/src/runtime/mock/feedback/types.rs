//! Data types for the `feedback` module.
#[allow(unused_imports)]
use super::*;
/// The scripted board: items plus their comments, keyed by item id.
#[derive(Default)]
pub(in super::super) struct MockBoard {
    /// Board items, in insertion order.
    pub(in super::super) items: Vec<FeedbackItem>,
    /// Comments per item id, oldest first.
    pub(in super::super) comments: Vec<(String, Vec<FeedbackComment>)>,
}
/// The scripted description of one board row, before it is expanded into a
/// [`FeedbackItem`]. Grouping these keeps [`row`] to a single argument.
pub(super) struct Seed<'a> {
    /// The item id.
    pub(super) id: &'a str,
    /// Feature request or bug report.
    pub(super) kind: FeedbackType,
    /// Triage status.
    pub(super) status: FeedbackStatus,
    /// The item title.
    pub(super) title: &'a str,
    /// The item body.
    pub(super) body: &'a str,
    /// Upvotes.
    pub(super) up: i64,
    /// Downvotes.
    pub(super) down: i64,
    /// The demo user's own vote.
    pub(super) my_vote: i8,
    /// How many comments the item has.
    pub(super) comment_count: i64,
    /// The filed GitHub issue number, when the item has been filed.
    pub(super) issue: Option<i64>,
}
