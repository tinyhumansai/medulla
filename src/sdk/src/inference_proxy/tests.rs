//! Unit tests for the loopback attribution proxy: the header rewrite's strip and
//! inject rules, mount/URL mapping, OpenRouter host recognition, the router
//! rewrite, and token minting.
//!
//! Everything here is offline. The socket-level behaviour (streaming, real
//! forwarding, 401s) is covered by `src/sdk/tests/e2e_attribution_proxy.rs`.

use std::collections::HashMap;

use http::header::{HeaderMap, HeaderName, HeaderValue};

use super::routing::{is_openrouter, resolve_key};
use super::*;
use crate::config::{RouterConfig, RouterProviderConfig};
use crate::tinyplace::HarnessProvider;

/// Build a header map from `(name, value)` pairs.
fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            HeaderName::from_bytes(name.as_bytes()).expect("test header name"),
            HeaderValue::from_str(value).expect("test header value"),
        );
    }
    map
}

/// A router pointing every provider at OpenRouter through the top-level endpoint.
fn openrouter_router() -> RouterConfig {
    RouterConfig {
        base_url: Some(OPENROUTER_ROOT.to_string()),
        api_key_env: Some("OPENROUTER_API_KEY".to_string()),
        ..RouterConfig::default()
    }
}

fn endpoint() -> ProxyEndpoint {
    ProxyEndpoint {
        port: 4242,
        token: "mdl-testtoken".to_string(),
    }
}

#[test]
fn rewrite_replaces_harness_attribution_with_medulla() {
    let inbound = headers(&[
        ("http-referer", "https://claude.ai"),
        ("x-title", "Claude Code"),
    ]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");

    assert_eq!(out.get("http-referer").unwrap(), MEDULLA_REFERER);
    assert_eq!(out.get("x-title").unwrap(), MEDULLA_TITLE);
    // Exactly one of each — an `append` bug would leave the harness's claim in
    // place alongside ours and OpenRouter would read the first.
    assert_eq!(out.get_all("http-referer").iter().count(), 1);
    assert_eq!(out.get_all("x-title").iter().count(), 1);
}

#[test]
fn rewrite_strips_the_standard_referer_spelling_too() {
    let inbound = headers(&[("referer", "https://opencode.ai")]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");
    assert!(out.get("referer").is_none());
}

#[test]
fn rewrite_strips_namespaced_attribution_headers() {
    let inbound = headers(&[
        ("x-openrouter-app", "codex"),
        ("x-app-name", "opencode"),
        ("x-app-version", "1.2.3"),
    ]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");
    assert!(out.get("x-openrouter-app").is_none());
    assert!(out.get("x-app-name").is_none());
    assert!(out.get("x-app-version").is_none());
}

#[test]
fn rewrite_substitutes_the_upstream_credential() {
    let inbound = headers(&[
        ("authorization", "Bearer mdl-localtoken"),
        ("x-api-key", "mdl-localtoken"),
    ]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");

    assert_eq!(out.get("authorization").unwrap(), "Bearer sk-or-real");
    // The loopback token must not reach OpenRouter under any spelling.
    assert!(out.get("x-api-key").is_none());
    assert_eq!(out.get_all("authorization").iter().count(), 1);
}

#[test]
fn rewrite_marks_the_credential_sensitive() {
    let out = rewrite(&HeaderMap::new(), "sk-or-real").expect("valid key");
    assert!(out.get("authorization").unwrap().is_sensitive());
}

#[test]
fn rewrite_forwards_protocol_headers_untouched() {
    let inbound = headers(&[
        ("anthropic-version", "2023-06-01"),
        ("anthropic-beta", "prompt-caching-2024-07-31"),
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
        ("user-agent", "claude-cli/2.0.0"),
    ]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");

    assert_eq!(out.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(
        out.get("anthropic-beta").unwrap(),
        "prompt-caching-2024-07-31"
    );
    assert_eq!(out.get("content-type").unwrap(), "application/json");
    assert_eq!(out.get("accept").unwrap(), "text/event-stream");
    // User-Agent is deliberately preserved: attribution is the referer/title
    // pair, and rewriting the UA would misreport the client to the upstream.
    assert_eq!(out.get("user-agent").unwrap(), "claude-cli/2.0.0");
}

#[test]
fn rewrite_drops_connection_scoped_headers() {
    let inbound = headers(&[
        ("host", "127.0.0.1:4242"),
        ("connection", "keep-alive"),
        ("transfer-encoding", "chunked"),
        ("content-length", "128"),
        ("proxy-connection", "keep-alive"),
        ("te", "trailers"),
        ("upgrade", "h2c"),
    ]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");

    for name in [
        "host",
        "connection",
        "transfer-encoding",
        "content-length",
        "proxy-connection",
        "te",
        "upgrade",
    ] {
        assert!(out.get(name).is_none(), "{name} should be stripped");
    }
}

#[test]
fn rewrite_preserves_repeated_values_of_forwarded_headers() {
    let inbound = headers(&[("anthropic-beta", "one"), ("anthropic-beta", "two")]);
    let out = rewrite(&inbound, "sk-or-real").expect("valid key");
    assert_eq!(out.get_all("anthropic-beta").iter().count(), 2);
}

#[test]
fn rewrite_refuses_a_key_that_cannot_be_a_header_value() {
    assert!(rewrite(&HeaderMap::new(), "bad\nkey").is_none());
}

#[test]
fn openrouter_hosts_are_recognized_by_host_not_prefix() {
    assert!(is_openrouter("https://openrouter.ai/api"));
    assert!(is_openrouter("https://openrouter.ai/api/v1"));
    assert!(is_openrouter("http://openrouter.ai:8080/api"));
    assert!(is_openrouter("https://www.openrouter.ai/api"));
    assert!(is_openrouter("https://OpenRouter.AI/api"));
}

#[test]
fn lookalike_and_unrelated_hosts_are_not_openrouter() {
    // A subdomain suffix attack: prefix matching on the URL would accept this.
    assert!(!is_openrouter("https://openrouter.ai.example.com/api"));
    assert!(!is_openrouter("https://evil.com/openrouter.ai/api"));
    assert!(!is_openrouter("https://api.anthropic.com"));
    assert!(!is_openrouter("http://127.0.0.1:8080/v1"));
}

#[test]
fn each_provider_is_mounted_on_its_own_dialect() {
    for (provider, expected) in [
        (HarnessProvider::Claude, "http://127.0.0.1:4242/anthropic"),
        (HarnessProvider::Codex, "http://127.0.0.1:4242/openai"),
        (HarnessProvider::Opencode, "http://127.0.0.1:4242/openai"),
    ] {
        let routing = route_openrouter(
            &openrouter_router(),
            provider,
            &endpoint(),
            "OPENROUTER_API_KEY",
        )
        .expect("routed");

        assert_eq!(
            routing
                .router
                .providers
                .get(provider.as_str())
                .unwrap()
                .base_url
                .as_deref(),
            Some(expected)
        );
        assert_eq!(routing.router.providers.len(), 1);
    }
}

#[test]
fn routing_swaps_the_key_name_for_the_token_name() {
    let routing = route_openrouter(
        &openrouter_router(),
        HarnessProvider::Claude,
        &endpoint(),
        "OPENROUTER_API_KEY",
    )
    .expect("routed");

    assert_eq!(routing.router.api_key_env.as_deref(), Some(PROXY_TOKEN_ENV));
    assert_eq!(routing.router.base_url.as_deref(), Some(OPENROUTER_ROOT));
    assert_eq!(
        routing.env,
        vec![(PROXY_TOKEN_ENV.to_string(), "mdl-testtoken".to_string())]
    );
}

#[test]
fn routing_scrubs_both_the_configured_and_default_key_names() {
    let mut router = openrouter_router();
    router.api_key_env = Some("MY_OR_KEY".to_string());
    let routing = route_openrouter(&router, HarnessProvider::Claude, &endpoint(), "MY_OR_KEY")
        .expect("routed");

    assert!(routing.scrub_env.contains(&"MY_OR_KEY".to_string()));
    assert!(routing
        .scrub_env
        .contains(&"OPENROUTER_API_KEY".to_string()));
}

#[test]
fn routing_scrub_list_has_no_duplicates_for_the_default_name() {
    let routing = route_openrouter(
        &openrouter_router(),
        HarnessProvider::Claude,
        &endpoint(),
        "OPENROUTER_API_KEY",
    )
    .expect("routed");
    assert_eq!(routing.scrub_env, vec!["OPENROUTER_API_KEY".to_string()]);
}

#[test]
fn a_non_openrouter_router_is_left_alone() {
    let router = RouterConfig {
        base_url: Some("https://gateway.internal/v1".to_string()),
        api_key_env: Some("GATEWAY_KEY".to_string()),
        ..RouterConfig::default()
    };
    assert!(
        route_openrouter(&router, HarnessProvider::Claude, &endpoint(), "GATEWAY_KEY").is_none()
    );
}

#[test]
fn an_unconfigured_router_is_left_alone() {
    assert!(route_openrouter(
        &RouterConfig::default(),
        HarnessProvider::Claude,
        &endpoint(),
        "OPENROUTER_API_KEY"
    )
    .is_none());
}

#[test]
fn another_providers_openrouter_override_does_not_route_this_spawn() {
    let mut router = RouterConfig {
        api_key_env: Some("OPENROUTER_API_KEY".to_string()),
        ..RouterConfig::default()
    };
    router.providers.insert(
        "claude".to_string(),
        RouterProviderConfig {
            base_url: Some(OPENROUTER_ROOT.to_string()),
        },
    );
    assert!(route_openrouter(
        &router,
        HarnessProvider::Codex,
        &endpoint(),
        "OPENROUTER_API_KEY"
    )
    .is_none());
}

#[test]
fn another_providers_override_does_not_scrub_this_spawns_key() {
    let mut configured = RouterConfig {
        api_key_env: Some("OPENROUTER_API_KEY".to_string()),
        ..RouterConfig::default()
    };
    configured.providers.insert(
        "claude".to_string(),
        RouterProviderConfig {
            base_url: Some(OPENROUTER_ROOT.to_string()),
        },
    );
    let mut router = Some(configured.clone());
    let mut env = HashMap::from([("OPENROUTER_API_KEY".to_string(), "sk-or-real".to_string())]);

    route_spawn(HarnessProvider::Codex, &mut router, &mut env).expect("no-op");

    assert_eq!(router, Some(configured));
    assert_eq!(
        env.get("OPENROUTER_API_KEY").map(String::as_str),
        Some("sk-or-real")
    );
    assert!(!env.contains_key(PROXY_TOKEN_ENV));
}

#[test]
fn a_provider_override_alone_is_enough_to_route() {
    let mut router = RouterConfig {
        api_key_env: Some("OPENROUTER_API_KEY".to_string()),
        ..RouterConfig::default()
    };
    router.providers.insert(
        "claude".to_string(),
        RouterProviderConfig {
            base_url: Some(OPENROUTER_ROOT.to_string()),
        },
    );
    let routing = route_openrouter(
        &router,
        HarnessProvider::Claude,
        &endpoint(),
        "OPENROUTER_API_KEY",
    )
    .expect("routed");

    assert_eq!(
        routing
            .router
            .providers
            .get("claude")
            .unwrap()
            .base_url
            .as_deref(),
        Some("http://127.0.0.1:4242/anthropic")
    );
    // Untouched providers gain no entry, so they keep falling through to their
    // own on-disk configuration.
    assert!(!routing.router.providers.contains_key("codex"));
}

#[test]
fn models_survive_the_rewrite() {
    let mut router = openrouter_router();
    router.models.insert(
        "reasoning".to_string(),
        "anthropic/claude-opus-4".to_string(),
    );
    let routing = route_openrouter(
        &router,
        HarnessProvider::Claude,
        &endpoint(),
        "OPENROUTER_API_KEY",
    )
    .expect("routed");

    assert_eq!(
        routing.router.model_for_tier("reasoning"),
        Some("anthropic/claude-opus-4")
    );
}

#[test]
fn route_spawn_scrubs_the_real_key_and_rewrites_the_router() {
    let mut router = Some(openrouter_router());
    let mut env: HashMap<String, String> = HashMap::from([
        ("OPENROUTER_API_KEY".to_string(), "sk-or-real".to_string()),
        ("PATH".to_string(), "/usr/bin".to_string()),
        // Keeps the listener off the network: the run never reaches upstream in
        // this test, but starting one that could would be wrong regardless.
        (
            UPSTREAM_URL_ENV.to_string(),
            "http://127.0.0.1:1/api".to_string(),
        ),
    ]);
    route_spawn(HarnessProvider::Claude, &mut router, &mut env).expect("routed");

    // The real key is gone — a harness in this environment has nothing to reach
    // OpenRouter with except the loopback token.
    assert!(!env.contains_key("OPENROUTER_API_KEY"));
    let token = env.get(PROXY_TOKEN_ENV).expect("token exported");
    assert!(token.starts_with("mdl-"));
    assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));

    let router = router.expect("router kept");
    assert_eq!(router.api_key_env.as_deref(), Some(PROXY_TOKEN_ENV));
    assert!(router
        .base_url_for("claude")
        .expect("claude routed")
        .starts_with("http://127.0.0.1:"));
}

#[test]
fn route_spawn_leaves_an_unrouted_run_untouched() {
    let mut router = None;
    let mut env: HashMap<String, String> =
        HashMap::from([("OPENROUTER_API_KEY".to_string(), "sk-or-real".to_string())]);
    route_spawn(HarnessProvider::Claude, &mut router, &mut env).expect("no-op");

    assert!(router.is_none());
    // Nothing scrubbed: with no `[router]` the key is the harness's own business.
    assert_eq!(
        env.get("OPENROUTER_API_KEY").map(String::as_str),
        Some("sk-or-real")
    );
}

#[test]
fn key_resolution_falls_back_to_the_documented_default() {
    let router = RouterConfig {
        base_url: Some(OPENROUTER_ROOT.to_string()),
        ..RouterConfig::default()
    };
    let env = HashMap::from([("OPENROUTER_API_KEY".to_string(), "sk-or-real".to_string())]);
    let resolved = resolve_key(&router, &env).expect("resolved");
    assert_eq!(resolved.0, "OPENROUTER_API_KEY");
    assert_eq!(resolved.1, "sk-or-real");
}

#[test]
fn a_blank_or_absent_key_resolves_to_nothing() {
    let router = openrouter_router();
    assert!(resolve_key(&router, &HashMap::new()).is_none());
    let blank = HashMap::from([("OPENROUTER_API_KEY".to_string(), "   ".to_string())]);
    assert!(resolve_key(&router, &blank).is_none());
}

#[test]
fn a_run_without_a_key_is_not_routed() {
    // No key exported → `Ok(None)`, and crucially no listener is started, so an
    // otherwise-working direct run is left exactly as it was.
    let routed = route_run(
        &openrouter_router(),
        HarnessProvider::Claude,
        &HashMap::new(),
    )
    .expect("no error");
    assert!(routed.is_none());
}

#[test]
fn mounts_map_onto_their_upstream_dialects() {
    assert_eq!(
        UpstreamShape::Anthropic.upstream_base(OPENROUTER_ROOT),
        "https://openrouter.ai/api"
    );
    assert_eq!(
        UpstreamShape::OpenAi.upstream_base(OPENROUTER_ROOT),
        "https://openrouter.ai/api/v1"
    );
    // A trailing slash on the configured root must not double up.
    assert_eq!(
        UpstreamShape::OpenAi.upstream_base("https://openrouter.ai/api/"),
        "https://openrouter.ai/api/v1"
    );
}

#[test]
fn paths_split_into_a_mount_and_a_remainder() {
    use super::serve::split_mount;

    assert_eq!(
        split_mount("/anthropic/v1/messages"),
        Some((UpstreamShape::Anthropic, "/v1/messages"))
    );
    assert_eq!(
        split_mount("/openai/chat/completions"),
        Some((UpstreamShape::OpenAi, "/chat/completions"))
    );
    assert_eq!(
        split_mount("/anthropic"),
        Some((UpstreamShape::Anthropic, ""))
    );
    assert_eq!(split_mount("/unknown/v1"), None);
    assert_eq!(split_mount("/"), None);
}

#[test]
fn upstream_urls_carry_the_query_string() {
    use super::serve::upstream_url;

    assert_eq!(
        upstream_url(
            OPENROUTER_ROOT,
            UpstreamShape::Anthropic,
            "/v1/messages",
            None
        ),
        "https://openrouter.ai/api/v1/messages"
    );
    assert_eq!(
        upstream_url(
            OPENROUTER_ROOT,
            UpstreamShape::OpenAi,
            "/models",
            Some("limit=5")
        ),
        "https://openrouter.ai/api/v1/models?limit=5"
    );
}

#[test]
fn tokens_are_minted_once_per_credential() {
    let handle = ProxyHandle::start(OPENROUTER_ROOT.to_string()).expect("proxy starts");

    let first = handle.endpoint_for_key("sk-or-one");
    let again = handle.endpoint_for_key("sk-or-one");
    let other = handle.endpoint_for_key("sk-or-two");

    assert_eq!(first.token, again.token, "same key reuses its token");
    assert_ne!(
        first.token, other.token,
        "distinct keys get distinct tokens"
    );
    assert!(first.token.starts_with("mdl-"));
    assert_eq!(first.port, handle.port());
    assert_ne!(handle.port(), 0);
}
