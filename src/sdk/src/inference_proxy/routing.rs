//! Provider-scoped router and child-environment rewriting for proxy launches.

use std::collections::HashMap;

use crate::config::{RouterConfig, RouterProviderConfig, OPENROUTER_API_KEY_ENV};
use crate::tinyplace::HarnessProvider;

use super::lifecycle::shared;
use super::types::{ProxyEndpoint, ProxyRouting, UpstreamShape, PROXY_TOKEN_ENV};

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

/// Resolve the OpenRouter credential named by `router` from a run environment.
pub(super) fn resolve_key(
    router: &RouterConfig,
    provider: HarnessProvider,
    env: &HashMap<String, String>,
) -> Option<(String, String)> {
    if let Some(name) = router
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let value = env.get(name).map(|value| value.trim())?;
        return (!value.is_empty()).then(|| (name.to_string(), value.to_string()));
    }

    // Endpoint-only router configuration intentionally leaves a harness's own
    // credentials in place. Prefer the documented OpenRouter variable, then the
    // native variable the selected harness would inherit for this dialect.
    let provider_key = match provider {
        HarnessProvider::Claude => "ANTHROPIC_AUTH_TOKEN",
        HarnessProvider::Codex | HarnessProvider::Opencode => "OPENAI_API_KEY",
    };
    [OPENROUTER_API_KEY_ENV, provider_key]
        .into_iter()
        .find_map(|name| {
            env.get(name)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(|value| (name.to_string(), value.to_string()))
        })
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
