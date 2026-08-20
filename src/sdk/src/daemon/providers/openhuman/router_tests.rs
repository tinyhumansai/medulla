use std::collections::HashMap;

use super::embedded_route;
use crate::config::{RouterConfig, OPENROUTER_API_KEY_ENV, OPENROUTER_OPENAI_URL};

/// The router an `openhuman` preset produces: OpenRouter's OpenAI-shaped
/// endpoint and the default key variable.
fn openrouter_router() -> RouterConfig {
    RouterConfig {
        base_url: Some(OPENROUTER_OPENAI_URL.to_string()),
        api_key_env: Some(OPENROUTER_API_KEY_ENV.to_string()),
        ..RouterConfig::default()
    }
}

fn env_with(name: &str, value: &str) -> HashMap<String, String> {
    HashMap::from([(name.to_string(), value.to_string())])
}

#[test]
fn no_router_leaves_the_turn_on_the_account_configuration() {
    let route = embedded_route(
        None,
        &env_with(OPENROUTER_API_KEY_ENV, "sk-or-v1"),
        Some("x/y"),
    )
    .expect("resolving a route cannot fail without a router");
    assert!(route.is_none());
}

#[test]
fn a_missing_key_is_not_an_error() {
    // The documented shape of an OpenHuman preset that names only a model: it
    // stays usable on a machine that never exported an OpenRouter key, and the
    // turn runs on the core's own provider bindings.
    let route = embedded_route(Some(&openrouter_router()), &HashMap::new(), Some("x/y"))
        .expect("an absent key is a routing decision, not a failure");
    assert!(route.is_none());
}

#[test]
fn a_blank_key_counts_as_absent() {
    let route = embedded_route(
        Some(&openrouter_router()),
        &env_with(OPENROUTER_API_KEY_ENV, "   "),
        Some("x/y"),
    )
    .expect("a blank key is a routing decision, not a failure");
    assert!(route.is_none());
}

#[test]
fn an_endpoint_that_is_not_openrouter_reaches_the_core_unproxied() {
    // The proxy exists to own OpenRouter attribution. A preset pointed at some
    // other OpenAI-compatible host — a local router, a self-hosted gateway — has
    // nothing to attribute, so it is handed to the core as spelled rather than
    // diverted through a mount that forwards to openrouter.ai.
    let router = RouterConfig {
        base_url: Some("http://127.0.0.1:6969/v1".to_string()),
        api_key_env: Some("LOCAL_ROUTER_KEY".to_string()),
        ..RouterConfig::default()
    };
    let route = embedded_route(
        Some(&router),
        &env_with("LOCAL_ROUTER_KEY", "local-secret"),
        Some("flash"),
    )
    .expect("a direct endpoint starts no listener and cannot fail")
    .expect("an endpoint with a key routes");

    assert_eq!(route.base_url, "http://127.0.0.1:6969/v1");
    assert_eq!(route.token, "local-secret");
}

#[test]
fn a_direct_endpoint_without_its_key_stays_on_the_account_configuration() {
    // Same rule as the OpenRouter shape: a preset naming a variable nobody
    // exported is not a failure, it is a turn that runs where it always did.
    let router = RouterConfig {
        base_url: Some("http://127.0.0.1:6969/v1".to_string()),
        api_key_env: Some("LOCAL_ROUTER_KEY".to_string()),
        ..RouterConfig::default()
    };
    let route = embedded_route(Some(&router), &HashMap::new(), Some("flash"))
        .expect("an absent key is a routing decision, not a failure");
    assert!(route.is_none());
}

#[test]
fn a_router_naming_no_endpoint_is_not_routed() {
    // With no endpoint there is nothing to point the core at, and defaulting to
    // one would send the turn somewhere the preset never named.
    let router = RouterConfig {
        api_key_env: Some(OPENROUTER_API_KEY_ENV.to_string()),
        ..RouterConfig::default()
    };
    let route = embedded_route(
        Some(&router),
        &env_with(OPENROUTER_API_KEY_ENV, "sk-or-v1"),
        Some("x/y"),
    )
    .expect("an endpointless router is a routing decision, not a failure");
    assert!(route.is_none());
}

#[test]
fn a_turn_with_no_model_is_not_routed() {
    // The core expresses a per-call route as a provider/model pair and ignores
    // one it cannot pin to a model. Deciding that here keeps the two sides
    // agreeing instead of sending a request the core discards.
    for model in [None, Some(""), Some("   ")] {
        let route = embedded_route(
            Some(&openrouter_router()),
            &env_with(OPENROUTER_API_KEY_ENV, "sk-or-v1"),
            model,
        )
        .expect("an unresolved model is a routing decision, not a failure");
        assert!(route.is_none(), "model {model:?} should not route");
    }
}

#[test]
fn a_routed_turn_gets_a_loopback_mount_and_a_token_not_the_key() {
    let key = "sk-or-v1-embedded-route";
    let route = embedded_route(
        Some(&openrouter_router()),
        &env_with(OPENROUTER_API_KEY_ENV, key),
        Some("anthropic/claude-sonnet-4"),
    )
    .expect("the shared proxy starts")
    .expect("an OpenRouter endpoint with a key routes");

    // Bound to the literal loopback address and the OpenAI-shaped mount, which
    // is the dialect the core speaks.
    assert!(
        route.base_url.starts_with("http://127.0.0.1:"),
        "unexpected base url {}",
        route.base_url
    );
    assert!(
        route.base_url.ends_with("/openai"),
        "unexpected mount {}",
        route.base_url
    );

    // The whole point of the exchange: what the core is handed authenticates
    // against this process and is worthless at openrouter.ai.
    assert!(route.token.starts_with("mdl-"));
    assert_ne!(route.token, key);
}

#[test]
fn a_presets_own_key_variable_is_honoured() {
    let router = RouterConfig {
        base_url: Some(OPENROUTER_OPENAI_URL.to_string()),
        api_key_env: Some("TEAM_OPENROUTER_KEY".to_string()),
        ..RouterConfig::default()
    };
    // The named variable is authoritative: the ambient default must not stand in
    // for it, or a preset would silently spend the wrong account's credit.
    let ambient = env_with(OPENROUTER_API_KEY_ENV, "sk-or-v1-ambient");
    assert!(embedded_route(Some(&router), &ambient, Some("x/y"))
        .expect("an absent named key is not a failure")
        .is_none());

    let named = env_with("TEAM_OPENROUTER_KEY", "sk-or-v1-team");
    assert!(embedded_route(Some(&router), &named, Some("x/y"))
        .expect("the shared proxy starts")
        .is_some());
}

#[test]
fn one_credential_reuses_one_token() {
    // Token minting is per upstream key, not per run, so a host dispatching many
    // turns on one preset does not grow the proxy's registry.
    let env = env_with(OPENROUTER_API_KEY_ENV, "sk-or-v1-stable");
    let first = embedded_route(Some(&openrouter_router()), &env, Some("x/y"))
        .expect("the shared proxy starts")
        .expect("routes");
    let second = embedded_route(Some(&openrouter_router()), &env, Some("x/y"))
        .expect("the shared proxy starts")
        .expect("routes");
    assert_eq!(first, second);
}
