//! End-to-end tests for the loopback attribution proxy, observed from the far
//! side.
//!
//! The unit tests in `src/sdk/src/inference_proxy/tests.rs` prove the header
//! rewrite is correct as a function. These prove it actually reaches OpenRouter:
//! a real socket, a real forward, and assertions made on what the *upstream*
//! received. A rewrite that were dropped during the forward would pass the unit
//! tests and fail here.
//!
//! Offline throughout — the "upstream" is a mock bound to loopback.

use std::time::Duration;

use medulla::inference_proxy::{ProxyHandle, MEDULLA_REFERER, MEDULLA_TITLE};

#[path = "support/mock_openrouter.rs"]
mod mock_openrouter;

use mock_openrouter::{MockOpenRouter, Reply};

/// The OpenRouter key the proxy holds and the child must never see.
const UPSTREAM_KEY: &str = "sk-or-v1-realsecret";

/// Start a mock upstream and a proxy pointed at it, returning both plus the
/// endpoint a child would be given.
async fn harness(reply: Reply) -> (MockOpenRouter, medulla::inference_proxy::ProxyEndpoint) {
    let upstream = MockOpenRouter::start(reply).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let endpoint = proxy.endpoint_for_key(UPSTREAM_KEY);
    (upstream, endpoint)
}

#[tokio::test]
async fn medulla_attribution_replaces_the_harness_on_the_wire() {
    let (upstream, endpoint) = harness(Reply::Json("{\"ok\":true}".to_string())).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        // Exactly what Claude Code would send: its own attribution claim.
        .header("HTTP-Referer", "https://claude.ai")
        .header("X-Title", "Claude Code")
        .header("x-openrouter-app", "claude-code")
        .header("anthropic-version", "2023-06-01")
        .body("{\"model\":\"anthropic/claude-opus-4\"}")
        .send()
        .await
        .expect("request reaches the proxy");
    assert!(response.status().is_success());

    let received = upstream.only_request();
    assert_eq!(received.header("http-referer"), Some(MEDULLA_REFERER));
    assert_eq!(received.header("x-title"), Some(MEDULLA_TITLE));
    // One value each: the harness's claim is replaced, not merely appended to.
    assert_eq!(received.header_count("http-referer"), 1);
    assert_eq!(received.header_count("x-title"), 1);
    assert_eq!(received.header("x-openrouter-app"), None);
    // The protocol header the harness needs answered survives untouched.
    assert_eq!(received.header("anthropic-version"), Some("2023-06-01"));
}

#[tokio::test]
async fn the_real_key_is_substituted_and_the_local_token_never_leaves() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;

    reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .header("x-api-key", &endpoint.token)
        .body("{}")
        .send()
        .await
        .expect("request reaches the proxy");

    let received = upstream.only_request();
    assert_eq!(
        received.header("authorization"),
        Some(format!("Bearer {UPSTREAM_KEY}").as_str())
    );
    // The loopback token is worthless off this machine, but it is still a
    // credential this process minted: it must not be forwarded under any name.
    let serialized = format!("{:?}", received.headers);
    assert!(
        !serialized.contains(&endpoint.token),
        "loopback token leaked upstream: {serialized}"
    );
}

#[tokio::test]
async fn the_request_body_survives_the_forward_intact() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;
    // Large enough to span several stream chunks, which is where a naive
    // forward loses or reorders data.
    let payload = format!("{{\"prompt\":\"{}\"}}", "abcdefghij".repeat(4096));

    reqwest::Client::new()
        .post(format!(
            "{}/chat/completions",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::OpenAi)
        ))
        .bearer_auth(&endpoint.token)
        .body(payload.clone())
        .send()
        .await
        .expect("request reaches the proxy");

    assert_eq!(upstream.only_request().body, payload);
}

#[tokio::test]
async fn each_mount_maps_onto_its_upstream_dialect() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;
    let client = reqwest::Client::new();

    client
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .body("{}")
        .send()
        .await
        .expect("anthropic mount");
    client
        .get(format!(
            "{}/models?limit=5",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::OpenAi)
        ))
        .bearer_auth(&endpoint.token)
        .send()
        .await
        .expect("openai mount");

    let targets: Vec<String> = upstream
        .requests()
        .into_iter()
        .map(|request| request.target)
        .collect();
    assert!(
        targets.contains(&"/api/v1/messages".to_string()),
        "anthropic mount should reach the API root: {targets:?}"
    );
    // The OpenAI dialect lands a `/v1` below, and the query string rides along.
    assert!(
        targets.contains(&"/api/v1/models?limit=5".to_string()),
        "openai mount should reach the versioned base: {targets:?}"
    );
}

#[tokio::test]
async fn an_unknown_token_is_refused_before_reaching_upstream() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth("mdl-not-a-real-token")
        .body("{}")
        .send()
        .await
        .expect("request reaches the proxy");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    // Refused locally: an unauthenticated caller must not cost an upstream
    // request, let alone spend the operator's credit.
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
async fn a_request_with_no_credential_is_refused() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .body("{}")
        .send()
        .await
        .expect("request reaches the proxy");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
async fn an_unknown_mount_is_not_forwarded() {
    let (upstream, endpoint) = harness(Reply::Json("{}".to_string())).await;

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/somewhere/else", endpoint.port))
        .bearer_auth(&endpoint.token)
        .body("{}")
        .send()
        .await
        .expect("request reaches the proxy");

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
async fn sse_responses_are_relayed_incrementally() {
    // The load-bearing property for a harness: a token stream must arrive as it
    // is produced. A proxy that collected the response first would still pass
    // every header assertion above while turning a live stream into one long
    // pause.
    let gap = Duration::from_millis(400);
    let (_upstream, endpoint) = harness(Reply::SlowStream {
        first: "data: {\"delta\":\"one\"}\n\n".to_string(),
        second: "data: {\"delta\":\"two\"}\n\n".to_string(),
        gap,
    })
    .await;

    let started = tokio::time::Instant::now();
    let mut response = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .body("{}")
        .send()
        .await
        .expect("request reaches the proxy");

    let first = response
        .chunk()
        .await
        .expect("stream readable")
        .expect("a first chunk");
    let first_at = started.elapsed();
    assert!(
        String::from_utf8_lossy(&first).contains("one"),
        "first chunk should carry the first event"
    );
    // Arrived well before the upstream even wrote the second event.
    assert!(
        first_at < gap / 2,
        "first chunk took {first_at:?}, which means the response was buffered"
    );

    let mut rest = Vec::new();
    while let Some(chunk) = response.chunk().await.expect("stream readable") {
        rest.extend_from_slice(&chunk);
    }
    assert!(
        String::from_utf8_lossy(&rest).contains("two"),
        "the rest of the stream should follow"
    );
    assert!(
        started.elapsed() >= gap,
        "the full stream cannot have completed before the upstream wrote it"
    );
}

#[tokio::test]
async fn distinct_credentials_do_not_share_a_token() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let first = proxy.endpoint_for_key("sk-or-one");
    let second = proxy.endpoint_for_key("sk-or-two");

    let client = reqwest::Client::new();
    for (endpoint, expected) in [(&first, "sk-or-one"), (&second, "sk-or-two")] {
        client
            .post(format!(
                "{}/v1/messages",
                endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
            ))
            .bearer_auth(&endpoint.token)
            .body("{}")
            .send()
            .await
            .expect("request reaches the proxy");
        let received = upstream.requests().pop().expect("a recorded request");
        assert_eq!(
            received.header("authorization"),
            Some(format!("Bearer {expected}").as_str()),
            "each token must resolve to its own upstream key"
        );
    }
}
