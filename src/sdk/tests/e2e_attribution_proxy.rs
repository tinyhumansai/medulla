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

use bytes::Bytes;
use futures::stream;
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

#[tokio::test]
async fn a_pinned_run_states_its_upstream_provider_in_the_forwarded_body() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let endpoint = proxy.endpoint_for_credential(UPSTREAM_KEY, &["streamlake".to_string()]);

    reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .header("content-type", "application/json")
        .body("{\"model\":\"z-ai/glm-5.2\",\"max_tokens\":16}")
        .send()
        .await
        .expect("request reaches the proxy");

    let received = upstream.only_request();
    let body: serde_json::Value =
        serde_json::from_str(&received.body).expect("forwarded body is JSON");
    assert_eq!(body["provider"]["only"], serde_json::json!(["streamlake"]));
    // The request the harness composed is otherwise untouched.
    assert_eq!(body["model"], "z-ai/glm-5.2");
    assert_eq!(body["max_tokens"], 16);
    // A rewritten body must not carry the pre-rewrite length.
    let length: usize = received
        .header("content-length")
        .expect("upstream is told the length")
        .parse()
        .expect("content-length is a number");
    assert_eq!(length, received.body.len());
}

#[tokio::test]
async fn an_unpinned_run_forwards_its_body_verbatim() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let endpoint = proxy.endpoint_for_key(UPSTREAM_KEY);

    reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .body("{\"model\":\"z-ai/glm-5.2\"}")
        .send()
        .await
        .expect("request reaches the proxy");

    let received = upstream.only_request();
    assert_eq!(received.body, "{\"model\":\"z-ai/glm-5.2\"}");
}

#[tokio::test]
async fn one_key_pinned_two_ways_does_not_leak_a_pin_across_presets() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    // The exact shape of two presets sharing `apiKeyEnv`: one pinned, one not.
    let pinned = proxy.endpoint_for_credential(UPSTREAM_KEY, &["streamlake".to_string()]);
    let unpinned = proxy.endpoint_for_key(UPSTREAM_KEY);
    assert_ne!(
        pinned.token, unpinned.token,
        "a pin is part of the credential's identity"
    );

    let client = reqwest::Client::new();
    for endpoint in [&pinned, &unpinned] {
        client
            .post(format!(
                "{}/v1/messages",
                endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
            ))
            .bearer_auth(&endpoint.token)
            .body("{\"model\":\"m\"}")
            .send()
            .await
            .expect("request reaches the proxy");
    }

    let requests = upstream.requests();
    let pinned_body: serde_json::Value =
        serde_json::from_str(&requests[0].body).expect("forwarded body is JSON");
    assert_eq!(
        pinned_body["provider"]["only"],
        serde_json::json!(["streamlake"])
    );
    assert_eq!(
        requests[1].body, "{\"model\":\"m\"}",
        "the unpinned preset's traffic must be untouched"
    );
}

#[tokio::test]
async fn a_pinned_request_over_the_rewrite_limit_is_streamed_without_the_pin() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let endpoint = proxy.endpoint_for_credential(UPSTREAM_KEY, &["streamlake".to_string()]);

    // One byte past the rewrite limit, so `forward_body` must not buffer it:
    // the safety valve streams the body unpinned rather than applying the pin.
    // Any smaller body would rewrite, and this test exists to pin the valve.
    let payload = format!(
        "{{\"model\":\"z-ai/glm-5.2\",\"prompt\":\"{}\"}}",
        "x".repeat(medulla::inference_proxy::MAX_REWRITE_BYTES)
    );

    reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .body(payload.clone())
        .send()
        .await
        .expect("request reaches the proxy");

    let received = upstream.only_request();
    assert_eq!(
        received.body, payload,
        "an oversized pinned body is forwarded verbatim"
    );
    let body: serde_json::Value =
        serde_json::from_str(&received.body).expect("forwarded body is JSON");
    assert!(
        body.get("provider").is_none(),
        "no provider pin is applied past the rewrite limit"
    );
}

#[tokio::test]
async fn a_chunked_pinned_request_is_not_buffered_past_the_rewrite_limit() {
    let upstream = MockOpenRouter::start(Reply::Json("{}".to_string())).await;
    let proxy = ProxyHandle::start(upstream.root.clone()).expect("proxy starts");
    let endpoint = proxy.endpoint_for_credential(UPSTREAM_KEY, &["streamlake".to_string()]);

    // Streamed with no Content-Length, so the request arrives chunked and the
    // proxy gets a size hint whose lower bound is 0. The size-hint fast path in
    // `forward_body` cannot catch this body; only the incremental frame read
    // may stop past the rewrite limit and stream the remainder unpinned. This
    // test exists to pin that branch, which the Content-Length variant above
    // never reaches.
    let payload = format!(
        "{{\"model\":\"z-ai/glm-5.2\",\"prompt\":\"{}\"}}",
        "z".repeat(medulla::inference_proxy::MAX_REWRITE_BYTES)
    );
    // Chunk well under the rewrite limit — half of it — so a payload one byte
    // over the limit always spans several frames no matter where the limit
    // sits. A fixed 1 MiB chunk would silently collapse to a single frame (and
    // never reach the limit mid-stream, which is the very branch this test
    // exists to pin) if `MAX_REWRITE_BYTES` were ever lowered below it.
    let chunk_size = (medulla::inference_proxy::MAX_REWRITE_BYTES / 2).max(1);
    let frames: Vec<Result<Bytes, std::io::Error>> = payload
        .as_bytes()
        .chunks(chunk_size)
        .map(Bytes::copy_from_slice)
        .map(Ok)
        .collect();
    let frame_count = frames.len();
    assert!(
        frame_count > 1,
        "the payload must span several chunks to trip the limit mid-stream"
    );

    reqwest::Client::new()
        .post(format!(
            "{}/v1/messages",
            endpoint.base_url(medulla::inference_proxy::UpstreamShape::Anthropic)
        ))
        .bearer_auth(&endpoint.token)
        .header("content-type", "application/json")
        .body(reqwest::Body::wrap_stream(stream::iter(frames)))
        .send()
        .await
        .expect("request reaches the proxy");

    let received = upstream.only_request();
    eprintln!(
        "DBG chunked received.len={} payload.len={} frames={} firstdiff follow",
        received.body.len(),
        payload.len(),
        frame_count
    );
    assert_eq!(
        received.body, payload,
        "an oversized chunked body is forwarded verbatim"
    );
    let body: serde_json::Value =
        serde_json::from_str(&received.body).expect("forwarded body is JSON");
    assert!(
        body.get("provider").is_none(),
        "no provider pin is applied past the rewrite limit"
    );
}
