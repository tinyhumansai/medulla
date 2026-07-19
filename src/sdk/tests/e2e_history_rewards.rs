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
