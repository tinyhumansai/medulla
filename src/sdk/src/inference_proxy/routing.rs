//! Provider-scoped router and child-environment rewriting for proxy launches.

use std::collections::HashMap;

use crate::config::{RouterConfig, RouterProviderConfig, OPENROUTER_API_KEY_ENV};
use crate::protocol::HarnessProvider;

use super::lifecycle::shared;
use super::types::{EmbeddedRouting, ProxyEndpoint, ProxyRouting, UpstreamShape, PROXY_TOKEN_ENV};

/// Whether `base_url` points at OpenRouter.
///
/// Host matching accepts harmless scheme, port, and path differences without
/// accepting a lookalike domain such as `openrouter.ai.example.com`.
pub(super) fn is_openrouter(base_url: &str) -> bool {
    let without_scheme = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host.split(':').next().unwrap_or_default();
    host.eq_ignore_ascii_case("openrouter.ai") || host.eq_ignore_ascii_case("www.openrouter.ai")
}

/// Rewrite the selected provider when it resolves to OpenRouter.
///
/// This is pure and provider-scoped: unrelated provider overrides cannot cause
/// a Codex or OpenCode launch to receive a proxy token intended for Claude.
/// Returns `None` when the selected provider is not OpenRouter-bound.
pub fn route_openrouter(
    router: &RouterConfig,
    provider: HarnessProvider,
    proxy: &ProxyEndpoint,
    api_key_env: &str,
) -> Option<ProxyRouting> {
    let id = provider.as_str();
    let base_url = router.base_url_for(id)?;
    if !is_openrouter(base_url) {
        return None;
    }

    let mut providers = router.providers.clone();
    providers.insert(
        id.to_string(),
        RouterProviderConfig {
            base_url: Some(proxy.base_url(UpstreamShape::for_provider(provider))),
        },
    );

    // Both names are scrubbed because an ambient default key can coexist with
    // a preset's custom variable and would otherwise reopen the direct bypass.
    let mut scrub_env = vec![OPENROUTER_API_KEY_ENV.to_string()];
    if !api_key_env.is_empty() && api_key_env != OPENROUTER_API_KEY_ENV {
        scrub_env.push(api_key_env.to_string());
    }

    Some(ProxyRouting {
        router: RouterConfig {
            base_url: router.base_url.clone(),
            api_key_env: Some(PROXY_TOKEN_ENV.to_string()),
            models: router.models.clone(),
            providers,
        },
        env: vec![(PROXY_TOKEN_ENV.to_string(), proxy.token.clone())],
        scrub_env,
    })
}

/// The variable `router` names for its credential, when it names one.
///
/// A configured name is authoritative: an absent value under it means "no key",
/// not "look somewhere else". Only an endpoint-only router falls through to the
/// conventional names.
fn configured_key_env(router: &RouterConfig) -> Option<&str> {
    router
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// Read `name` out of `env`, treating a blank value as absent.
fn env_key(env: &HashMap<String, String>, name: &str) -> Option<(String, String)> {
    env.get(name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| (name.to_string(), value.to_string()))
}

/// Resolve the OpenRouter credential named by `router` from a run environment.
pub(super) fn resolve_key(
    router: &RouterConfig,
    provider: HarnessProvider,
    env: &HashMap<String, String>,
) -> Option<(String, String)> {
    // The embedded core is not a child and never reaches a spawn seam; it routes
    // through [`route_embedded`] instead, which resolves its own credential.
    if provider == HarnessProvider::Openhuman {
        return None;
    }
    if let Some(name) = configured_key_env(router) {
        return env_key(env, name);
    }

    // Endpoint-only router configuration intentionally leaves a harness's own
    // credentials in place. Prefer the documented OpenRouter variable, then the
    // native variable the selected harness would inherit for this dialect.
    let provider_key = match provider {
        HarnessProvider::Claude => "ANTHROPIC_AUTH_TOKEN",
        HarnessProvider::Codex | HarnessProvider::Opencode => "OPENAI_API_KEY",
        HarnessProvider::Openhuman => return None,
    };
    [OPENROUTER_API_KEY_ENV, provider_key]
        .into_iter()
        .find_map(|name| env_key(env, name))
}

/// Resolve the credential for an in-process run.
///
/// Same precedence as [`resolve_key`] minus its last step: there is no child to
/// inherit a harness's native variable, so `ANTHROPIC_AUTH_TOKEN` and
/// `OPENAI_API_KEY` are not consulted. A preset's `apiKeyEnv` wins, and an
/// endpoint-only router falls back to the documented OpenRouter variable.
fn resolve_embedded_key(
    router: &RouterConfig,
    env: &HashMap<String, String>,
) -> Option<(String, String)> {
    match configured_key_env(router) {
        Some(name) => env_key(env, name),
        None => env_key(env, OPENROUTER_API_KEY_ENV),
    }
}

/// Route the embedded core's inference through the shared proxy, starting it on
/// demand.
///
/// Returns `Ok(None)` when the run's endpoint is not OpenRouter-bound or the
/// named key is absent — in either case the core keeps resolving its own
/// provider bindings from the account's configuration, which is what an
/// operator who configured neither expects.
///
/// # Errors
///
/// The sentence [`shared`] produces when the loopback listener cannot bind.
pub fn route_embedded(
    router: &RouterConfig,
    env: &HashMap<String, String>,
) -> Result<Option<EmbeddedRouting>, String> {
    let provider = HarnessProvider::Openhuman;
    if !router
        .base_url_for(provider.as_str())
        .is_some_and(is_openrouter)
    {
        return Ok(None);
    }
    let Some((_, key)) = resolve_embedded_key(router, env) else {
        return Ok(None);
    };
    let endpoint = shared(env)?.endpoint_for_key(&key);
    Ok(Some(EmbeddedRouting {
        base_url: endpoint.base_url(UpstreamShape::for_provider(provider)),
        token: endpoint.token,
    }))
}

/// Route the selected provider through the shared proxy, starting it on demand.
///
/// Returns `Ok(None)` when that provider is not OpenRouter-bound or the named
/// key is absent, preserving the original launch behavior in either case.
pub fn route_run(
    router: &RouterConfig,
    provider: HarnessProvider,
    env: &HashMap<String, String>,
) -> Result<Option<ProxyRouting>, String> {
    let Some((name, key)) = resolve_key(router, provider, env) else {
        return Ok(None);
    };
    let probe = ProxyEndpoint {
        port: 0,
        token: String::new(),
    };
    if route_openrouter(router, provider, &probe, &name).is_none() {
        return Ok(None);
    }
    let endpoint = shared(env)?.endpoint_for_key(&key);
    Ok(route_openrouter(router, provider, &endpoint, &name))
}

/// Route one provider spawn's router and environment through the proxy in place.
///
/// Router rewrite, token injection, and real-key scrubbing stay atomic at every
/// spawn seam. This is a no-op for providers that are not OpenRouter-bound.
pub fn route_spawn(
    provider: HarnessProvider,
    router: &mut Option<RouterConfig>,
    env: &mut HashMap<String, String>,
) -> Result<(), String> {
    let Some(configured) = router.as_ref() else {
        return Ok(());
    };
    let Some(routing) = route_run(configured, provider, env)? else {
        return Ok(());
    };
    for name in &routing.scrub_env {
        env.remove(name);
    }
    for (name, value) in &routing.env {
        env.insert(name.clone(), value.clone());
    }
    *router = Some(routing.router);
    Ok(())
}
