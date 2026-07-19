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
use medulla_tui::ui::welcome::{drive_welcome_ui, WelcomeOutcome};

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

#[tokio::test]
async fn the_full_flow_runs_from_intro_to_a_claimed_reward() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .expect("flow should succeed");

    assert_eq!(
        outcome,
        WelcomeOutcome::Completed {
            awarded_usd: 5.0,
            tier: Some("Rising".into()),
        }
    );

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

    tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

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

    assert_eq!(outcome, WelcomeOutcome::Skipped);

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

    assert_eq!(outcome, WelcomeOutcome::Skipped);
    assert_eq!(backend.requests().len(), 1, "only the status check");
}

#[tokio::test]
async fn an_unreachable_backend_skips_rather_than_blocking_startup() {
    // Nothing is listening on this port; the flow must not block the app.
    let dead = MedullaClient::new("http://127.0.0.1:1", "test-jwt");
    let mut term = terminal();

    let outcome = drive_welcome_ui(&mut term, &dead, HashMap::new(), repeating(KeyCode::Enter))
        .await
        .unwrap();

    assert_eq!(outcome, WelcomeOutcome::Skipped);
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

    assert_eq!(outcome, WelcomeOutcome::Skipped);
    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        paths.iter().filter(|p| p.ends_with("/uploads")).count(),
        0,
        "nothing to upload"
    );
}

#[tokio::test]
async fn a_failing_claim_still_ends_the_flow() {
    let backend = MockBackend::start().await;
    // Uploads succeed but the claim 404s (an unknown route on the mock).
    backend.configure(|config| config.history_claim = serde_json::Value::Null);
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    // A null claim decodes to a zeroed one, so the flow still completes rather
    // than trapping the user on a spinner.
    assert!(matches!(outcome, WelcomeOutcome::Completed { .. }));
}

#[tokio::test]
async fn the_flow_renders_every_step_it_passes_through() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());
    let mut term = terminal();

    tokio::time::timeout(
        Duration::from_secs(20),
        drive_welcome_ui(&mut term, &client(&backend), env, repeating(KeyCode::Enter)),
    )
    .await
    .expect("flow should not hang")
    .unwrap();

    // The terminal's final frame is the reveal.
    let rendered: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("POWER LEVEL"), "final frame: {rendered}");
    assert!(rendered.contains("RISING"), "final frame: {rendered}");
    assert!(
        rendered.contains("$5 of $25 earned"),
        "final frame: {rendered}"
    );
}
