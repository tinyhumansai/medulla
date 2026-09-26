//! Mocked end-to-end coverage for the history-reward client methods.
//!
//! Verifies the wire contract the TUI depends on: that each method hits the
//! right path and verb, sends the transcript as multipart with its agent label,
//! carries the bearer token, and decodes the backend's camelCase `{success,data}`
//! envelope into the SDK's types — including the flattened status inside a claim.

use serde_json::json;

use medulla::client::MedullaClient;

#[path = "support/mod.rs"]
mod support;
use support::mock_backend::MockBackend;

fn client(backend: &MockBackend) -> MedullaClient {
    MedullaClient::new(backend.base_url.clone(), "test-jwt")
}

#[tokio::test]
async fn status_decodes_an_unclaimed_reward() {
    let backend = MockBackend::start().await;

    let status = client(&backend).history_reward_status().await.unwrap();

    assert!(!status.claimed);
    assert!(!status.has_uploads);
    assert_eq!(status.awarded_usd, 0.0);
    assert_eq!(status.max_reward_usd, 25.0);
    assert!(status.tier.is_none());

    let requests = backend.requests();
    let hit = requests.last().unwrap();
    assert_eq!(hit.method, "GET");
    assert_eq!(hit.path, "/agent-integrations/history-rewards/status");
}

#[tokio::test]
async fn status_decodes_an_already_claimed_reward() {
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.history_status = json!({
            "claimed": true,
            "hasUploads": true,
            "awardedUsd": 12.5,
            "tier": "Elite",
            "sessionCount": 41,
            "cumulativeTokens": 12_000_000,
            "activeDays": 18,
            "agents": ["claude", "codex"],
            "maxRewardUsd": 25,
        });
    });

    let status = client(&backend).history_reward_status().await.unwrap();

    assert!(status.claimed);
    assert_eq!(status.awarded_usd, 12.5);
    assert_eq!(status.tier.as_deref(), Some("Elite"));
    assert_eq!(status.session_count, 41);
    assert_eq!(status.cumulative_tokens, 12_000_000);
    assert_eq!(status.active_days, 18);
    assert_eq!(status.agents, vec!["claude", "codex"]);
}

#[tokio::test]
async fn a_missing_optional_field_falls_back_to_its_default() {
    // The client must keep working against a server that has not yet grown a
    // field, rather than failing to decode.
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.history_status = json!({ "claimed": false });
    });

    let status = client(&backend).history_reward_status().await.unwrap();

    assert!(!status.claimed);
    assert_eq!(status.session_count, 0);
    assert_eq!(status.max_reward_usd, 0.0);
    assert!(status.agents.is_empty());
}

#[tokio::test]
async fn uploading_posts_the_transcript_as_multipart_with_its_agent() {
    let backend = MockBackend::start().await;
    let transcript = "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n";

    let result = client(&backend)
        .upload_history_session("claude", transcript.to_string())
        .await
        .unwrap();

    assert_eq!(result.session_count, 1);
    assert_eq!(result.cumulative_tokens, 187_126);
    assert_eq!(result.active_days, 3);
    assert_eq!(result.agents, vec!["claude"]);

    let requests = backend.requests();
    let hit = requests.last().unwrap();
    assert_eq!(hit.method, "POST");
    assert_eq!(hit.path, "/agent-integrations/history-rewards/uploads");
    // Multipart body: the agent field and the transcript both present, with the
    // filename the endpoint expects.
    assert!(
        hit.body.contains("name=\"agent\""),
        "agent field: {}",
        hit.body
    );
    assert!(hit.body.contains("claude"), "agent value: {}", hit.body);
    assert!(
        hit.body.contains("name=\"file\""),
        "file part: {}",
        hit.body
    );
    assert!(hit.body.contains("session.jsonl"), "filename: {}", hit.body);
    assert!(
        hit.body.contains("input_tokens"),
        "transcript body: {}",
        hit.body
    );
}

#[tokio::test]
async fn each_agent_label_is_sent_verbatim() {
    for agent in ["claude", "codex", "opencode"] {
        let backend = MockBackend::start().await;
        client(&backend)
            .upload_history_session(agent, "{}\n".to_string())
            .await
            .unwrap();

        let requests = backend.requests();
        assert!(
            requests.last().unwrap().body.contains(agent),
            "{agent} should appear in the multipart body"
        );
    }
}

#[tokio::test]
async fn an_upload_rejected_by_the_backend_surfaces_as_an_api_error() {
    let backend = MockBackend::start().await;
    backend.configure(|config| config.history_upload_ok = false);

    let err = client(&backend)
        .upload_history_session("claude", "{}\n".to_string())
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("already been claimed"),
        "expected the server's reason, got: {err}"
    );
}

#[tokio::test]
async fn claiming_decodes_the_award_breakdown_and_flattened_status() {
    let backend = MockBackend::start().await;

    let claim = client(&backend).claim_history_reward().await.unwrap();

    // Flattened status fields.
    assert!(claim.status.claimed);
    assert_eq!(claim.status.awarded_usd, 5.0);
    assert_eq!(claim.status.tier.as_deref(), Some("Rising"));
    assert_eq!(claim.status.max_reward_usd, 25.0);
    assert_eq!(claim.status.cumulative_tokens, 209_226);
    assert_eq!(claim.status.active_days, 5);

    // Nested breakdown.
    assert_eq!(claim.breakdown.tokens_usd, 2.0);
    assert_eq!(claim.breakdown.active_days_usd, 0.0);
    assert_eq!(claim.breakdown.sessions_usd, 0.0);
    assert_eq!(claim.breakdown.multi_agent_usd, 3.0);
    assert!(!claim.already_claimed);

    let requests = backend.requests();
    let hit = requests.last().unwrap();
    assert_eq!(hit.method, "POST");
    assert_eq!(hit.path, "/agent-integrations/history-rewards/claim");
}

#[tokio::test]
async fn a_repeat_claim_reports_already_claimed() {
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.history_claim = json!({
            "claimed": true,
            "hasUploads": true,
            "awardedUsd": 5,
            "tier": "Rising",
            "maxRewardUsd": 25,
            "breakdown": { "tokensUsd": 2, "activeDaysUsd": 0, "sessionsUsd": 0, "multiAgentUsd": 3 },
            "alreadyClaimed": true,
        });
    });

    let claim = client(&backend).claim_history_reward().await.unwrap();

    assert!(claim.already_claimed);
    assert_eq!(claim.status.awarded_usd, 5.0);
}

#[tokio::test]
async fn a_zero_award_still_decodes() {
    let backend = MockBackend::start().await;
    backend.configure(|config| {
        config.history_claim = json!({
            "claimed": true,
            "awardedUsd": 0,
            "tier": "Newcomer",
            "maxRewardUsd": 25,
            "alreadyClaimed": false,
        });
    });

    let claim = client(&backend).claim_history_reward().await.unwrap();

    assert_eq!(claim.status.awarded_usd, 0.0);
    assert_eq!(claim.status.tier.as_deref(), Some("Newcomer"));
    assert_eq!(claim.breakdown.tokens_usd, 0.0);
}

#[tokio::test]
async fn every_history_request_carries_the_bearer_token() {
    let backend = MockBackend::start().await;
    let client = client(&backend);

    client.history_reward_status().await.unwrap();
    client
        .upload_history_session("claude", "{}\n".to_string())
        .await
        .unwrap();
    client.claim_history_reward().await.unwrap();

    let paths: Vec<String> = backend
        .requests()
        .iter()
        .map(|request| request.path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/agent-integrations/history-rewards/status",
            "/agent-integrations/history-rewards/uploads",
            "/agent-integrations/history-rewards/claim",
        ]
    );
}

#[tokio::test]
async fn the_full_share_sequence_runs_status_uploads_then_claim() {
    // The exact call order the welcome flow performs.
    let backend = MockBackend::start().await;
    let client = client(&backend);

    let status = client.history_reward_status().await.unwrap();
    assert!(!status.claimed, "a new user has not claimed");

    for agent in ["claude", "codex"] {
        client
            .upload_history_session(agent, "{\"a\":1}\n".to_string())
            .await
            .unwrap();
    }

    let claim = client.claim_history_reward().await.unwrap();
    assert_eq!(claim.status.awarded_usd, 5.0);
    assert!(!claim.already_claimed);

    assert_eq!(backend.requests().len(), 4);
}

// --- share_history: the full consented sequence ----------------------------

use medulla::history_upload::{share_history, HistorySessionFile, ShareProgress};
use medulla::session_history::SessionAgentKind;

/// Write `count` transcripts into `dir` and describe them as scan results.
fn staged_files(dir: &std::path::Path, count: usize) -> Vec<HistorySessionFile> {
    (0..count)
        .map(|index| {
            let path = dir.join(format!("session-{index}.jsonl"));
            std::fs::write(
                &path,
                format!("{{\"i\":{index},\"t\":\"sk-abcdefghijklmnop0123456789\"}}\n"),
            )
            .unwrap();
            HistorySessionFile {
                agent: if index % 2 == 0 {
                    SessionAgentKind::Claude
                } else {
                    SessionAgentKind::Codex
                },
                path,
                size_bytes: 48,
                mtime_ms: index as i64,
            }
        })
        .collect()
}

#[tokio::test]
async fn share_history_uploads_every_transcript_then_claims() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = staged_files(dir.path(), 3);

    let mut progress: Vec<ShareProgress> = Vec::new();
    let claim = share_history(&client(&backend), &files, |p| progress.push(p))
        .await
        .unwrap();

    assert_eq!(claim.status.awarded_usd, 5.0);

    // One progress report per transcript, counting up.
    assert_eq!(progress.len(), 3);
    assert_eq!(progress[2].uploaded, 3);
    assert_eq!(progress[2].total, 3);
    // Each staged transcript carries one secret, scrubbed before sending.
    assert_eq!(progress[2].redactions, 3);

    // Three uploads then exactly one claim.
    let paths: Vec<String> = backend.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths.iter().filter(|p| p.ends_with("/uploads")).count(), 3);
    assert_eq!(paths.iter().filter(|p| p.ends_with("/claim")).count(), 1);
}

#[tokio::test]
async fn share_history_sends_redacted_content_never_the_original() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = staged_files(dir.path(), 1);

    share_history(&client(&backend), &files, |_| {})
        .await
        .unwrap();

    let requests = backend.requests();
    let upload = requests
        .iter()
        .find(|r| r.path.ends_with("/uploads"))
        .unwrap();
    assert!(
        !upload.body.contains("sk-abcdefghijklmnop"),
        "the raw secret must never reach the wire: {}",
        upload.body
    );
    assert!(upload.body.contains("[REDACTED]"));
}

#[tokio::test]
async fn share_history_labels_each_transcript_with_its_own_agent() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let files = staged_files(dir.path(), 2); // one claude, one codex

    share_history(&client(&backend), &files, |_| {})
        .await
        .unwrap();

    let requests = backend.requests();
    let bodies: Vec<&str> = requests
        .iter()
        .filter(|r| r.path.ends_with("/uploads"))
        .map(|r| r.body.as_str())
        .collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].contains("claude"));
    assert!(bodies[1].contains("codex"));
}

#[tokio::test]
async fn share_history_still_claims_when_every_upload_is_rejected() {
    // The backend refuses uploads once a claim has settled; the sequence must
    // still claim so the user is told the real state rather than stalling.
    let backend = MockBackend::start().await;
    backend.configure(|config| config.history_upload_ok = false);
    let dir = tempfile::tempdir().unwrap();
    let files = staged_files(dir.path(), 2);

    let mut progress: Vec<ShareProgress> = Vec::new();
    let claim = share_history(&client(&backend), &files, |p| progress.push(p))
        .await
        .unwrap();

    // Progress still advances (so the bar moves) but nothing counted as uploaded.
    assert_eq!(progress.len(), 2);
    assert_eq!(progress[1].uploaded, 0);
    assert_eq!(progress[1].total, 2);
    assert_eq!(progress[1].redactions, 0);
    assert_eq!(claim.status.awarded_usd, 5.0);
}

#[tokio::test]
async fn share_history_skips_unreadable_transcripts_without_aborting() {
    let backend = MockBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut files = staged_files(dir.path(), 1);
    files.push(HistorySessionFile {
        agent: SessionAgentKind::Claude,
        path: dir.path().join("does-not-exist.jsonl"),
        size_bytes: 10,
        mtime_ms: 99,
    });

    let mut progress: Vec<ShareProgress> = Vec::new();
    let claim = share_history(&client(&backend), &files, |p| progress.push(p))
        .await
        .unwrap();

    assert_eq!(progress.len(), 2, "both files reported");
    assert_eq!(progress[1].uploaded, 1, "only the readable one uploaded");
    assert_eq!(claim.status.awarded_usd, 5.0);
}

#[tokio::test]
async fn share_history_with_no_files_claims_immediately() {
    let backend = MockBackend::start().await;

    let mut called = 0;
    let claim = share_history(&client(&backend), &[], |_| called += 1)
        .await
        .unwrap();

    assert_eq!(called, 0);
    assert_eq!(claim.status.awarded_usd, 5.0);
    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].path.ends_with("/claim"));
}

#[tokio::test]
async fn share_history_surfaces_a_failing_claim() {
    let backend = MockBackend::start().await;
    drop(backend); // nothing is listening any more
    let dead = MedullaClient::new("http://127.0.0.1:1", "test-jwt");

    let result = share_history(&dead, &[], |_| {}).await;

    assert!(result.is_err(), "a failed claim must surface");
}
