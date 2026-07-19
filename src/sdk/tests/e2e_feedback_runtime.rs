//! Mocked end-to-end coverage for the feedback board as the UI reaches it —
//! through [`BackendRuntime`]'s `Runtime` methods rather than the raw client.
//!
//! The runtime's job here is delegation: turn a `Runtime` call into the right
//! client call and hand back the decoded result. These tests pin that seam, and
//! in particular that `list_feedback` answers `Some(page)` on a backend-backed
//! runtime — the value the UI uses to tell "no board" apart from "empty board".

use medulla::client::{FeedbackQuery, FeedbackSort, FeedbackType, MedullaClient};
use medulla::runtime::backend::BackendRuntime;
use medulla::runtime::Runtime;

#[path = "support/mod.rs"]
mod support;
use support::mock_backend::MockBackend;

async fn runtime(backend: &MockBackend) -> BackendRuntime {
    BackendRuntime::connect(MedullaClient::new(backend.base_url.clone(), "test-jwt"))
        .await
        .expect("the mock serves session creation")
}

#[tokio::test]
async fn listing_returns_a_page_rather_than_the_no_board_signal() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let page = runtime
        .list_feedback(FeedbackQuery::default())
        .await
        .expect("listing succeeds")
        .expect("a backend-backed runtime has a board");

    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "f1");
    assert_eq!(page.items[0].title, "Dark mode");
    assert_eq!(page.items[0].kind, FeedbackType::Feature);
}

#[tokio::test]
async fn the_query_is_carried_through_to_the_request() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let query = FeedbackQuery {
        kind: Some(FeedbackType::Bug),
        sort: FeedbackSort::Top,
        page: 3,
        ..FeedbackQuery::default()
    };
    runtime
        .list_feedback(query)
        .await
        .expect("listing succeeds");

    let requests = backend.requests();
    let listed = requests
        .iter()
        .rev()
        .find(|r| r.path.starts_with("/feedback"))
        .expect("a feedback request was made");
    assert!(listed.path.contains("bug"), "kind filter: {}", listed.path);
    assert!(listed.path.contains("3"), "page: {}", listed.path);
}

#[tokio::test]
async fn a_detail_request_returns_the_item_with_its_comments() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let detail = runtime
        .feedback_detail("f1".into())
        .await
        .expect("detail succeeds");

    assert_eq!(detail.feedback.id, "f1");
    assert_eq!(detail.comments.len(), 1);
    assert_eq!(detail.comments[0].body, "yes please");
    assert_eq!(detail.comments[0].user_name.as_deref(), Some("bob"));
}

#[tokio::test]
async fn voting_returns_the_item_with_recomputed_tallies() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let item = runtime.vote_feedback("f1".into(), 1).await.expect("vote");

    assert_eq!(item.my_vote, 1, "the caller's own vote comes back");
    assert_eq!(item.upvote_count, 4);
    assert_eq!(item.score, 3);

    let requests = backend.requests();
    assert!(
        requests.iter().any(|r| r.path.ends_with("/vote")),
        "a vote request was made"
    );
}

#[tokio::test]
async fn commenting_returns_the_posted_comment() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let comment = runtime
        .comment_feedback("f1".into(), "posted".into())
        .await
        .expect("comment");

    assert_eq!(comment.body, "posted");
    assert_eq!(comment.user_name.as_deref(), Some("ada"));
}

#[tokio::test]
async fn submitting_reports_acceptance() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let submission = runtime
        .submit_feedback(FeedbackType::Bug, "Crash".into(), "on resume".into())
        .await
        .expect("submit");

    assert!(submission.accepted, "the mock accepts the submission");
}

#[tokio::test]
async fn a_moderation_rejection_is_a_success_not_an_error() {
    // A rejected submission returns HTTP 200 with accepted == false and a
    // reason. Surfacing it as an error would lose the explanation the user
    // needs, so the runtime must pass it through as a successful call.
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.feedback_submission =
            serde_json::json!({ "accepted": false, "reason": "looks like spam" });
    });
    let runtime = runtime(&backend).await;

    let submission = runtime
        .submit_feedback(FeedbackType::Other, "spam".into(), "spam".into())
        .await
        .expect("a rejection resolves rather than erroring");

    assert!(!submission.accepted);
    assert_eq!(submission.reason, "looks like spam");
    assert!(submission.feedback.is_none(), "no item is created");
}
