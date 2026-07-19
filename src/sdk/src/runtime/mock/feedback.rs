//! A scripted, in-memory feedback board for [`MockRuntime`].
//!
//! `cargo run` with no credentials lands on the mock runtime, so this *is* the
//! offline demo of the Feedback tab. The board is stateful rather than a frozen
//! fixture: votes retally and comments persist for the life of the process, so
//! the tab's controls can be exercised without a backend.

use std::sync::{Arc, Mutex};

use crate::client::{
    FeedbackComment, FeedbackDetail, FeedbackGithub, FeedbackItem, FeedbackPage, FeedbackQuery,
    FeedbackStatus, FeedbackSubmission, FeedbackType,
};

use super::types::{gen_id, now_millis, MockRuntime};

/// The scripted board: items plus their comments, keyed by item id.
#[derive(Default)]
pub(super) struct MockBoard {
    /// Board items, in insertion order.
    pub(super) items: Vec<FeedbackItem>,
    /// Comments per item id, oldest first.
    pub(super) comments: Vec<(String, Vec<FeedbackComment>)>,
}

/// The scripted description of one board row, before it is expanded into a
/// [`FeedbackItem`]. Grouping these keeps [`row`] to a single argument.
struct Seed<'a> {
    /// The item id.
    id: &'a str,
    /// Feature request or bug report.
    kind: FeedbackType,
    /// Triage status.
    status: FeedbackStatus,
    /// The item title.
    title: &'a str,
    /// The item body.
    body: &'a str,
    /// Upvotes.
    up: i64,
    /// Downvotes.
    down: i64,
    /// The demo user's own vote.
    my_vote: i8,
    /// How many comments the item has.
    comment_count: i64,
    /// The filed GitHub issue number, when the item has been filed.
    issue: Option<i64>,
}

impl Default for Seed<'_> {
    fn default() -> Self {
        Seed {
            id: "",
            kind: FeedbackType::Feature,
            status: FeedbackStatus::Open,
            title: "",
            body: "",
            up: 0,
            down: 0,
            my_vote: 0,
            comment_count: 0,
            issue: None,
        }
    }
}

/// Expand a [`Seed`] into a scripted board row.
fn row(seed: Seed<'_>) -> FeedbackItem {
    FeedbackItem {
        id: seed.id.into(),
        kind: seed.kind,
        title: seed.title.into(),
        body: seed.body.into(),
        status: seed.status,
        created_by_name: Some("demo user".into()),
        upvote_count: seed.up,
        downvote_count: seed.down,
        score: seed.up - seed.down,
        comment_count: seed.comment_count,
        github: seed.issue.map(|n| FeedbackGithub {
            issue_number: Some(n),
            issue_url: Some(format!(
                "https://github.com/tinyhumansai/medulla/issues/{n}"
            )),
        }),
        my_vote: seed.my_vote,
        created_at: crate::ui::chat_store::iso8601_utc(now_millis()),
    }
}

/// One scripted comment.
fn comment(id: &str, who: &str, body: &str) -> FeedbackComment {
    FeedbackComment {
        id: id.into(),
        user_name: Some(who.into()),
        body: body.into(),
        created_at: crate::ui::chat_store::iso8601_utc(now_millis()),
    }
}

impl MockBoard {
    /// The board the offline demo starts with.
    pub(super) fn demo() -> Self {
        let items = vec![
            row(Seed {
                id: "fb-1",
                kind: FeedbackType::Feature,
                status: FeedbackStatus::Planned,
                title: "Split the Trace tab by agent lane",
                body: "Long cycles are hard to follow when every agent writes into one \
                       stream. Filtering the trace by lane would make debugging fan-out \
                       far easier.",
                up: 24,
                down: 1,
                my_vote: 1,
                comment_count: 2,
                issue: Some(412),
            }),
            row(Seed {
                id: "fb-2",
                kind: FeedbackType::Bug,
                status: FeedbackStatus::Open,
                title: "Resume picker forgets the active thread",
                body: "After resuming a chat the app lands on thread 1 instead of the \
                       thread that was active when the chat was saved.",
                up: 11,
                comment_count: 1,
                ..Default::default()
            }),
            row(Seed {
                id: "fb-3",
                kind: FeedbackType::Feature,
                status: FeedbackStatus::Completed,
                title: "Persist theme choice across restarts",
                body: "Appearance changes should survive a restart.",
                up: 8,
                down: 2,
                my_vote: -1,
                issue: Some(377),
                ..Default::default()
            }),
        ];
        let comments = vec![
            (
                "fb-1".to_string(),
                vec![
                    comment("c1", "avery", "Would pair well with per-lane token counts."),
                    comment(
                        "c2",
                        "demo user",
                        "Agreed — the Agents tab already has lanes.",
                    ),
                ],
            ),
            (
                "fb-2".to_string(),
                vec![comment("c3", "jules", "Reproduced on 0.3.1.")],
            ),
            ("fb-3".to_string(), Vec::new()),
        ];
        MockBoard { items, comments }
    }

    /// The comment list for `id`, inserting an empty one if absent.
    fn comments_mut(&mut self, id: &str) -> &mut Vec<FeedbackComment> {
        if !self.comments.iter().any(|(k, _)| k == id) {
            self.comments.push((id.to_string(), Vec::new()));
        }
        self.comments
            .iter_mut()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v)
            .expect("just inserted")
    }
}

impl MockRuntime {
    /// Apply a vote locally, mirroring the backend's retally-from-scratch rule:
    /// a repeated vote is not additive, and `0` retracts.
    pub(super) fn mock_vote(&self, id: &str, value: i8) -> anyhow::Result<FeedbackItem> {
        let mut board = self.board.lock().unwrap();
        let item = board
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| anyhow::anyhow!("Feedback not found"))?;

        // Undo the caller's previous vote, then apply the new one.
        match item.my_vote {
            1 => item.upvote_count -= 1,
            -1 => item.downvote_count -= 1,
            _ => {}
        }
        match value {
            1 => item.upvote_count += 1,
            -1 => item.downvote_count += 1,
            _ => {}
        }
        item.my_vote = value;
        item.score = item.upvote_count - item.downvote_count;
        Ok(item.clone())
    }

    /// Append a comment locally and bump the item's comment count.
    pub(super) fn mock_comment(&self, id: &str, body: &str) -> anyhow::Result<FeedbackComment> {
        let mut board = self.board.lock().unwrap();
        if !board.items.iter().any(|i| i.id == id) {
            return Err(anyhow::anyhow!("Feedback not found"));
        }
        let created = comment(&gen_id("c"), "you", body);
        board.comments_mut(id).push(created.clone());
        if let Some(item) = board.items.iter_mut().find(|i| i.id == id) {
            item.comment_count += 1;
        }
        Ok(created)
    }

    /// Add a submitted item to the head of the scripted board.
    pub(super) fn mock_submit(
        &self,
        kind: FeedbackType,
        title: &str,
        body: &str,
    ) -> FeedbackSubmission {
        let item = row(Seed {
            id: &gen_id("fb"),
            kind,
            title,
            body,
            ..Default::default()
        });
        self.board.lock().unwrap().items.insert(0, item.clone());
        FeedbackSubmission {
            accepted: true,
            reason: "accepted (mock runtime — nothing was sent to the backend)".into(),
            feedback: Some(item),
        }
    }

    /// Filter, sort, and paginate the scripted board like the backend does.
    pub(super) fn mock_list(&self, query: &FeedbackQuery) -> FeedbackPage {
        let board = self.board.lock().unwrap();
        let mut items: Vec<FeedbackItem> = board
            .items
            .iter()
            .filter(|i| query.kind.is_none_or(|k| k == i.kind))
            .filter(|i| query.status.is_none_or(|s| s == i.status))
            .cloned()
            .collect();
        // `hot` and `top` both order by score here; the mock has no time decay.
        match query.sort {
            crate::client::FeedbackSort::New => {}
            _ => items.sort_by_key(|i| std::cmp::Reverse(i.score)),
        }
        let total = items.len() as i64;
        let skip = ((query.page.max(1) - 1) * query.limit.max(1)) as usize;
        let items = items
            .into_iter()
            .skip(skip)
            .take(query.limit.clamp(1, 100) as usize)
            .collect();
        FeedbackPage { items, total }
    }

    /// One scripted item with its comments.
    pub(super) fn mock_detail(&self, id: &str) -> anyhow::Result<FeedbackDetail> {
        let board = self.board.lock().unwrap();
        let feedback = board
            .items
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Feedback not found"))?;
        let comments = board
            .comments
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        Ok(FeedbackDetail { feedback, comments })
    }
}

/// A fresh demo board handle.
pub(super) fn demo_board() -> Arc<Mutex<MockBoard>> {
    Arc::new(Mutex::new(MockBoard::demo()))
}
