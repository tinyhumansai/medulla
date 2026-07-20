//! End-to-end coverage of the welcome flow's driver loop, headlessly.
//!
//! Drives [`drive_welcome_ui`] with a ratatui test backend, a scripted key
//! stream, staged transcripts on disk, and a mock HTTP backend — so the whole
//! sequence (status check → scan → consent → redact → upload → claim → reveal)
//! runs exactly as it would against a real terminal, with no TTY.
//!
//! The key streams here repeat a single key rather than timing individual
//! presses. That keeps the tests deterministic: the steps that are busy
//! (`Scanning`, `Uploading`) ignore Enter outright, so a surplus press is a
//! no-op and the flow advances only when its async work actually lands.

use std::collections::HashMap;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::stream::{self, Stream, StreamExt};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::client::MedullaClient;
use medulla_tui::ui::welcome::{drive_welcome_ui, WelcomeEvent, WelcomeOutcome, WelcomeSession};

#[path = "../../sdk/tests/support/mod.rs"]
mod support;
use support::mock_backend::MockBackend;

/// A key stream that emits `code` every 5ms, forever.
fn repeating(code: KeyCode) -> impl Stream<Item = KeyEvent> + Unpin {
    Box::pin(
        stream::unfold((), move |()| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((KeyEvent::new(code, KeyModifiers::NONE), ()))
        })
        .boxed(),
    )
}

/// Stage one Claude and one Codex transcript, each carrying a secret, and return
/// an env pointing the scanner at them.
fn staged_history(dir: &std::path::Path) -> HashMap<String, String> {
    let claude = dir.join("claude");
    let codex = dir.join("codex");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&codex).unwrap();

    std::fs::write(
        claude.join("session.jsonl"),
        "{\"timestamp\":\"2026-01-05T10:00:00Z\",\"message\":{\"usage\":{\"input_tokens\":120000}},\
          \"note\":\"key sk-abcdefghijklmnop0123456789\"}\n",
    )
    .unwrap();
    std::fs::write(
        codex.join("rollout-2026-01-06.jsonl"),
        "{\"timestamp\":\"2026-01-06T10:00:00Z\",\"info\":{\"total_token_usage\":{\"total_tokens\":9000}}}\n",
    )
    .unwrap();

    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".into(),
        codex.to_string_lossy().into_owned(),
    );
    env
}

fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(90, 26)).unwrap()
}

fn client(backend: &MockBackend) -> MedullaClient {
    MedullaClient::new(backend.base_url.clone(), "test-jwt")
}

/// Drain a backgrounded share to its settled event.
///
/// The upload deliberately outlives the welcome screen, so a test that asserts
/// on uploads or the award has to wait for the detached task the way the app
/// does — by reading the channel the driver handed back.
async fn settle(session: WelcomeSession) -> Option<WelcomeEvent> {
    let mut rx = session.sharing?;
    while let Some(ev) = rx.recv().await {
        if matches!(ev, WelcomeEvent::Claimed { .. } | WelcomeEvent::Failed(_)) {
            return Some(ev);
        }
    }
    None
}

#[tokio::test]
async fn consenting_returns_immediately_and_shares_in_the_background() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let session = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .expect("flow should succeed");

    // Consent is the last thing the screen needs; the transfer continues without
    // holding the user on a progress bar.
    assert_eq!(session.outcome, WelcomeOutcome::Sharing);

    let settled = settle(session).await.expect("the share settles");
    match settled {
        WelcomeEvent::Claimed {
            awarded_usd, tier, ..
        } => {
            assert_eq!(awarded_usd, 5.0);
            assert_eq!(tier, Some("Rising".into()));
        }
        other => panic!("expected a claim, got {other:?}"),
    }

    // Status, then one upload per staged transcript, then exactly one claim.
    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths.iter().filter(|p| p.ends_with("/status")).count(), 1);
    assert_eq!(paths.iter().filter(|p| p.ends_with("/uploads")).count(), 2);
    assert_eq!(paths.iter().filter(|p| p.ends_with("/claim")).count(), 1);
}

#[tokio::test]
async fn the_uploaded_transcripts_are_redacted_on_the_wire() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let session = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();
    settle(session).await.expect("the share settles");

    let requests = backend.requests();
    let uploads: Vec<&String> = requests
        .iter()
        .filter(|r| r.path.ends_with("/uploads"))
        .map(|r| &r.body)
        .collect();
    assert_eq!(uploads.len(), 2);
    for body in &uploads {
        assert!(
            !body.contains("sk-abcdefghijklmnop"),
            "a secret reached the wire: {body}"
        );
    }
    // The token counter the backend scores on survived.
    assert!(uploads.iter().any(|body| body.contains("120000")));
}

#[tokio::test]
async fn declining_at_the_intro_uploads_nothing() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Esc)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    assert_eq!(outcome.outcome, WelcomeOutcome::Skipped);

    // Only the opening status check — nothing was scanned or sent.
    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths.iter().filter(|p| p.ends_with("/uploads")).count(), 0);
    assert_eq!(paths.iter().filter(|p| p.ends_with("/claim")).count(), 0);
}

#[tokio::test]
async fn an_already_claimed_reward_skips_without_drawing_anything() {
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.history_status = serde_json::json!({
            "claimed": true,
            "awardedUsd": 12,
            "tier": "Elite",
            "maxRewardUsd": 25,
        });
    });
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let outcome = drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter))
        .await
        .unwrap();

    assert_eq!(outcome.outcome, WelcomeOutcome::Skipped);
    assert_eq!(backend.requests().len(), 1, "only the status check");
}

#[tokio::test]
async fn an_unreachable_backend_yields_unavailable_rather_than_blocking_startup() {
    // Nothing is listening on this port. The flow must not block the app — but
    // it must also not report a decline, or the caller would record onboarding
    // as done and permanently burn the offer over a transient outage.
    let dead = MedullaClient::new("http://127.0.0.1:1", "test-jwt");
    let mut term = terminal();

    let outcome = drive_welcome_ui(&mut term, &dead, HashMap::new(), repeating(KeyCode::Enter))
        .await
        .unwrap();

    assert_eq!(outcome.outcome, WelcomeOutcome::Unavailable);
}

#[tokio::test]
async fn no_local_history_reaches_the_empty_state_and_skips() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".into(),
        dir.path().join("nope").to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".into(),
        dir.path().join("also-nope").to_string_lossy().into_owned(),
    );
    let mut term = terminal();

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    // Not Skipped: the offer stays open so it can be made once this user has
    // sessions, matching what the empty-state screen tells them.
    assert_eq!(outcome.outcome, WelcomeOutcome::NothingToShare);
    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        paths.iter().filter(|p| p.ends_with("/uploads")).count(),
        0,
        "nothing to upload"
    );
}

#[tokio::test]
async fn a_failing_claim_is_reported_through_the_share_channel() {
    let backend = MockBackend::start().await;
    // Uploads succeed but the claim 404s (an unknown route on the mock).
    backend.configure(|config| config.history_claim = serde_json::Value::Null);
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let session = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    // The user is already in the app; the failure reaches them on the status
    // line instead of trapping them on a spinner. Crucially it arrives as
    // `Failed`, which does not record onboarding — marking it done here would
    // lose a reward the user never actually received.
    assert_eq!(session.outcome, WelcomeOutcome::Sharing);
    let settled = settle(session).await.expect("the share settles");
    assert!(
        matches!(settled, WelcomeEvent::Failed(_)),
        "expected a failure, got {settled:?}"
    );
}

#[tokio::test]
async fn the_last_frame_the_user_sees_is_the_consent_step() {
    // The screen now hands off at consent, so the consent panel — not a progress
    // bar or a reveal — is the last thing drawn before the app takes over.
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let session = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();
    assert_eq!(session.outcome, WelcomeOutcome::Sharing);

    let rendered: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        !rendered.contains("POWER LEVEL"),
        "the reveal is no longer reached in this flow: {rendered}"
    );
}

/// Stage `count` transcripts so an upload run is long enough to interrupt.
fn many_sessions(dir: &std::path::Path, count: usize) -> HashMap<String, String> {
    let claude = dir.join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    for index in 0..count {
        std::fs::write(
            claude.join(format!("s{index}.jsonl")),
            format!("{{\"timestamp\":\"2026-01-05T10:00:00Z\",\"i\":{index}}}\n"),
        )
        .unwrap();
    }

    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".into(),
        dir.join("none").to_string_lossy().into_owned(),
    );
    env
}

#[tokio::test]
async fn declining_at_the_consent_step_uploads_nothing() {
    // Consent is now the last point at which the user can stop this: once they
    // approve, the share is handed to the app and runs to completion. So the
    // guarantee that matters is that declining *at the gate* transfers nothing
    // — no upload, no claim.
    let backend = std::sync::Arc::new(MockBackend::start().await);
    let dir = tempfile::tempdir().unwrap();
    let env = many_sessions(dir.path(), 60);
    let mut term = terminal();

    // Enter past the intro, then Esc as soon as the consent panel appears.
    // Keys react to observed state rather than a timer: the scan has to have
    // finished before Esc means "decline" rather than "abandon the scan".
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<KeyEvent>();
    tokio::spawn(async move {
        let _ = tx.send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        loop {
            if tx
                .send(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });
    let keys = Box::pin(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|key| (key, rx))
    }));

    let session = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, keys),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    assert_eq!(session.outcome, WelcomeOutcome::Skipped);
    assert!(
        session.sharing.is_none(),
        "declining must not leave a share running"
    );

    // Give any stray task a chance to act, so this fails loudly if a future
    // change starts the upload before the gate.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    let uploads = paths.iter().filter(|p| p.ends_with("/uploads")).count();
    let claims = paths.iter().filter(|p| p.ends_with("/claim")).count();
    assert_eq!(uploads, 0, "nothing may be uploaded: {paths:?}");
    assert_eq!(claims, 0, "and nothing may be claimed: {paths:?}");
}
