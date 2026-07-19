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

/// Build one scripted board row.
fn row(
    id: &str,
    kind: FeedbackType,
    status: FeedbackStatus,
    title: &str,
    body: &str,
    up: i64,
    down: i64,
    my_vote: i8,
    comment_count: i64,
    issue: Option<i64>,
) -> FeedbackItem {
    FeedbackItem {
        id: id.into(),
        kind,
        title: title.into(),
        body: body.into(),
        status,
        created_by_name: Some("demo user".into()),
        upvote_count: up,
        downvote_count: down,
        score: up - down,
        comment_count,
        github: issue.map(|n| FeedbackGithub {
            issue_number: Some(n),
            issue_url: Some(format!("https://github.com/tinyhumansai/medulla/issues/{n}")),
        }),
        my_vote,
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
            row(
                "fb-1",
                FeedbackType::Feature,
                FeedbackStatus::Planned,
                "Split the Trace tab by agent lane",
                "Long cycles are hard to follow when every agent writes into one \
                 stream. Filtering the trace by lane would make debugging fan-out \
                 far easier.",
                24,
                1,
                1,
                2,
                Some(412),
            ),
            row(
                "fb-2",
                FeedbackType::Bug,
                FeedbackStatus::Open,
                "Resume picker forgets the active thread",
                "After resuming a chat the app lands on thread 1 instead of the \
                 thread that was active when the chat was saved.",
                11,
                0,
                0,
                1,
                None,
            ),
            row(
                "fb-3",
                FeedbackType::Feature,
                FeedbackStatus::Completed,
                "Persist theme choice across restarts",
                "Appearance changes should survive a restart.",
                8,
                2,
                -1,
                0,
                Some(377),
            ),
        ];
        let comments = vec![
            (
                "fb-1".to_string(),
                vec![
                    comment("c1", "avery", "Would pair well with per-lane token counts."),
                    comment("c2", "demo user", "Agreed — the Agents tab already has lanes."),
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
        let item = row(
            &gen_id("fb"),
            kind,
            FeedbackStatus::Open,
            title,
            body,
            0,
            0,
            0,
            0,
            None,
        );
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
            _ => items.sort_by(|a, b| b.score.cmp(&a.score)),
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
