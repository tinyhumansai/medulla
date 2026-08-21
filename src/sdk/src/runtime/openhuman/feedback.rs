//! The feedback board half of [`OpenHumanRuntime`].
//!
//! The board is the one surface this runtime cannot serve from the core: it
//! lives on the cloud backend, behind the same session the core stores. So each
//! call mints a short-lived [`MedullaClient`] against the configured deployment
//! with the core's current token, rather than holding one built at construction
//! — the token can be replaced by a re-login while the app is running, and a
//! captured client would keep presenting the retired one.
//!
//! Reads degrade to "no board here" (`Ok(None)`), which the UI renders as a
//! sign-in hint; mutations fail loudly, because silently succeeding at a vote
//! that never reached the backend is worse than saying it did not.

use std::sync::Arc;

use futures::future::BoxFuture;
use openhuman_core::embed::Core;

use super::OpenHumanRuntime;
use crate::client::{
    FeedbackComment, FeedbackDetail, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSubmission,
    FeedbackType, MedullaClient,
};

impl OpenHumanRuntime {
    /// Build a backend client from the configured URL and the core's session.
    ///
    /// `Ok(None)` means this host has no board to talk to — unconfigured, or
    /// signed out. Callers decide whether that is an empty surface or an error.
    async fn feedback_client(
        core: Arc<Core>,
        base_url: Option<String>,
    ) -> anyhow::Result<Option<MedullaClient>> {
        let Some(base_url) = base_url else {
            return Ok(None);
        };
        let jwt = core
            .auth()
            .token()
            .await
            .map_err(|e| anyhow::anyhow!("could not read the stored session: {e}"))?;
        Ok(jwt.map(|jwt| MedullaClient::new(base_url, jwt)))
    }

    /// The client a board *mutation* needs, or the error explaining its absence.
    async fn feedback_client_or_err(
        core: Arc<Core>,
        base_url: Option<String>,
    ) -> anyhow::Result<MedullaClient> {
        Self::feedback_client(core, base_url).await?.ok_or_else(|| {
            anyhow::anyhow!("the feedback board requires a signed-in backend connection")
        })
    }

    /// The pieces every board call moves into its `'static` future.
    fn feedback_context(&self) -> (Arc<Core>, Option<String>) {
        (Arc::clone(&self.core), self.backend_base_url.clone())
    }

    /// One page of the board, or `None` when this host has no backend.
    pub(super) fn board_list(
        &self,
        query: FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<FeedbackPage>>> {
        let (core, base_url) = self.feedback_context();
        Box::pin(async move {
            let Some(client) = Self::feedback_client(core, base_url).await? else {
                return Ok(None);
            };
            Ok(Some(client.list_feedback(&query).await?))
        })
    }

    /// One board item with its comments.
    pub(super) fn board_detail(
        &self,
        id: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackDetail>> {
        let (core, base_url) = self.feedback_context();
        Box::pin(async move {
            let client = Self::feedback_client_or_err(core, base_url).await?;
            Ok(client.get_feedback(&id).await?)
        })
    }

    /// Cast, change, or retract a vote.
    pub(super) fn board_vote(
        &self,
        id: String,
        value: i8,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackItem>> {
        let (core, base_url) = self.feedback_context();
        Box::pin(async move {
            let client = Self::feedback_client_or_err(core, base_url).await?;
            Ok(client.vote_feedback(&id, value).await?)
        })
    }

    /// Post a comment on a board item.
    pub(super) fn board_comment(
        &self,
        id: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackComment>> {
        let (core, base_url) = self.feedback_context();
        Box::pin(async move {
            let client = Self::feedback_client_or_err(core, base_url).await?;
            Ok(client.comment_feedback(&id, &body).await?)
        })
    }

    /// Submit new feedback for moderation.
    pub(super) fn board_submit(
        &self,
        kind: FeedbackType,
        title: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackSubmission>> {
        let (core, base_url) = self.feedback_context();
        Box::pin(async move {
            let client = Self::feedback_client_or_err(core, base_url).await?;
            Ok(client.submit_feedback(kind, &title, &body).await?)
        })
    }
}
