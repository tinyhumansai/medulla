//! [`MedullaClient`] methods for the public feedback board.
//!
//! The board is the user-facing half of the backend's feedback surface: list,
//! read, vote, and comment, plus submission through the shared hub ingest
//! endpoint. Admin triage endpoints are deliberately not modelled here — they
//! are gated to operators and have no place in the TUI.
//!
//! Every call goes through [`tinyhumans_sdk::api::feedback::FeedbackApi`], so
//! path encoding, credential headers, and envelope unwrapping are the shared
//! transport's job. What stays here is the mapping between the SDK's open
//! `DynamicResponse` JSON and this crate's typed DTOs, which the UI is written
//! against and which carry the forward-compatible `Other` fallbacks the SDK's
//! stricter enums do not.

use tinyhumans_sdk::api::types::{
    FeedbackCommentRequest, FeedbackType as SdkFeedbackType, FeedbackVoteRequest,
    IngestFeedbackRequest,
};
use tinyhumans_sdk::QueryParam;

use super::types::{
    FeedbackComment, FeedbackDetail, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSubmission,
    FeedbackType,
};
use crate::client::{decode, MedullaClient, Result};

/// The source product this client submits feedback as. Drives which repository
/// the backend's enrichment pipeline files the resulting issue into.
const FEEDBACK_PRODUCT: &str = "medulla";

/// The `origin` recorded on submissions, distinguishing TUI reports from the
/// web board in backend analytics.
const FEEDBACK_ORIGIN: &str = "medulla-tui";

impl MedullaClient {
    // --- Feedback board (/feedback) --------------------------------------

    /// List the public feedback board (`GET /feedback`).
    ///
    /// Returns only publicly visible items (open/planned/completed); the
    /// backend filters pending and moderation-rejected items out server-side.
    /// Each item carries the caller's own `my_vote`.
    pub async fn list_feedback(&self, query: &FeedbackQuery) -> Result<FeedbackPage> {
        // A `kind`/`status` of `Other` is a forward-compat placeholder rather
        // than a filter the backend understands, so it is sent as no filter at
        // all — `wire()` answers `None` for it.
        let params: [QueryParam; 5] = [
            ("sort", Some(query.sort.as_str().to_string())),
            ("page", Some(query.page.max(1).to_string())),
            ("limit", Some(query.limit.clamp(1, 100).to_string())),
            (
                "type",
                query.kind.and_then(|k| k.wire()).map(str::to_string),
            ),
            (
                "status",
                query.status.and_then(|s| s.wire()).map(str::to_string),
            ),
        ];
        decode(self.sdk.feedback().list_feedback(&params).await?.0)
    }

    /// Fetch one board item with its comments (`GET /feedback/{id}`).
    pub async fn get_feedback(&self, id: &str) -> Result<FeedbackDetail> {
        decode(self.sdk.feedback().get_feedback(id).await?.0)
    }

    /// Vote on a board item (`POST /feedback/{id}/vote`).
    ///
    /// `value` is `1` to upvote, `-1` to downvote, or `0` to retract an existing
    /// vote. Returns the item with recomputed tallies. Values outside that set
    /// are rejected by the backend with a 400.
    pub async fn vote_feedback(&self, id: &str, value: i8) -> Result<FeedbackItem> {
        let response = self
            .sdk
            .feedback()
            .vote_feedback(id, &FeedbackVoteRequest { value })
            .await?;
        decode(response.0)
    }

    /// Comment on a board item (`POST /feedback/{id}/comments`).
    ///
    /// The backend rejects an empty body and caps length at 4000 characters.
    pub async fn comment_feedback(&self, id: &str, body: &str) -> Result<FeedbackComment> {
        let response = self
            .sdk
            .feedback()
            .comment_feedback(
                id,
                &FeedbackCommentRequest {
                    body: body.to_string(),
                },
            )
            .await?;
        decode(response.0)
    }

    /// Submit feedback through the shared hub (`POST /feedback/ingest`).
    ///
    /// Uses the ingest endpoint rather than `POST /feedback` so the item is
    /// tagged with the `medulla` source product and the backend routes any
    /// filed issue to the medulla repository. `POST /feedback` would hardcode
    /// the source as `backend` and misroute the issue.
    ///
    /// Submissions are LLM-moderated and rate-limited (10/day by default).
    /// A moderation rejection is **not** an error: it returns `Ok` with
    /// [`FeedbackSubmission::accepted`] set to false and a `reason`. A
    /// rate-limit breach *is* an error (HTTP 429).
    pub async fn submit_feedback(
        &self,
        kind: FeedbackType,
        title: &str,
        body: &str,
    ) -> Result<FeedbackSubmission> {
        let request = IngestFeedbackRequest {
            kind: sdk_kind(kind),
            title: title.to_string(),
            body: body.to_string(),
            product: FEEDBACK_PRODUCT.to_string(),
            origin: Some(FEEDBACK_ORIGIN.to_string()),
            external_ref: None,
        };
        decode(self.sdk.feedback().ingest_feedback(&request).await?.0)
    }
}

/// Map this crate's forward-compatible item type onto the SDK's closed enum.
///
/// `Other` exists only so a board row from a newer backend still decodes; it is
/// never something a user can pick, so submitting one falls back to a feature
/// request rather than failing the call.
fn sdk_kind(kind: FeedbackType) -> SdkFeedbackType {
    match kind {
        FeedbackType::Bug => SdkFeedbackType::Bug,
        FeedbackType::Feature | FeedbackType::Other => SdkFeedbackType::Feature,
    }
}
