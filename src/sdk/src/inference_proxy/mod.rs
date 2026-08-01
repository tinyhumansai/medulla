//! A loopback HTTP proxy that owns Medulla's OpenRouter attribution.
//!
//! # Why this exists
//!
//! When an operator supplies an OpenRouter key, Medulla points the harness at
//! OpenRouter through [`crate::config::RouterConfig`] and
//! [`crate::tinyplace::env::router_env`]. That works, but it hands OpenRouter a
//! request the *harness* composed — including the harness's own `HTTP-Referer`
//! and `X-Title`, the two headers OpenRouter reads to decide which application
//! to credit. Traffic Medulla orchestrated is therefore attributed to Claude
//! Code, Codex or OpenCode instead of to Medulla.
//!
//! Inserting a proxy Medulla controls is what makes the attribution ours: the
//! child is pointed at `127.0.0.1` instead of `openrouter.ai`, and every request
//! is re-headed on the way out (see [`headers::rewrite`]).
//!
//! # Why the child never sees the real key
//!
//! Rewriting headers is only worth anything if the harness cannot go around us.
//! So the child is given a **loopback token**, not the OpenRouter key: the token
//! authenticates against this process and is useless anywhere else, and the
//! spawn seam scrubs the real key out of the child environment
//! ([`types::ProxyRouting::scrub_env`]). A harness that tried to call OpenRouter
//! directly would have nothing to call it with.
//!
//! # Layout
//!
//! - [`headers`] — the pure request-header rewrite. All attribution policy.
//! - [`serve`] — the accept loop and the streaming forward.
//! - [`types`] — endpoint, dialect, and the router rewrite handed to a seam.
//!
//! # Known limitation
//!
//! This repo's boundary is env injection at the spawn seam; Medulla never writes
//! a harness's on-disk configuration. Attribution is therefore guaranteed for
//! Medulla-routed runs only. A harness the operator has separately configured to
//! reach OpenRouter through its own config file still bypasses this proxy.

mod headers;
mod serve;
mod types;

#[cfg(test)]
mod tests;

pub use headers::{rewrite, MEDULLA_REFERER, MEDULLA_TITLE};
pub use types::{
    ProxyEndpoint, ProxyRouting, UpstreamShape, OPENROUTER_ROOT, PROXY_TOKEN_ENV, UPSTREAM_URL_ENV,
};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::{RouterConfig, RouterProviderConfig, OPENROUTER_API_KEY_ENV};
use crate::tinyplace::HarnessProvider;

/// The loopback token ↔ upstream key mapping the serving loop authenticates
/// against.
///
/// Keyed both ways deliberately: `by_token` is the hot path (one lookup per
/// request), while `by_key` is what makes token minting idempotent, so two
/// presets sharing one OpenRouter key share one token instead of growing the
/// registry once per spawn.
#[derive(Debug, Default)]
struct CredentialRegistry {
    by_token: HashMap<String, String>,
    by_key: HashMap<String, String>,
}

/// A running proxy: the port it accepted on, plus the credentials it will honour.
///
/// One handle serves every harness in the process. It is cloneable and cheap —
/// the registry is shared, so a token minted through one clone is immediately
/// accepted by the serving loop.
#[derive(Debug, Clone)]
pub struct ProxyHandle {
    port: u16,
    registry: Arc<Mutex<CredentialRegistry>>,
}

impl ProxyHandle {
    /// Bind an ephemeral loopback port and start serving.
    ///
    /// `upstream_root` is the OpenRouter API root; tests pass a local mock so the
    /// suite stays offline. Synchronous, and safe to call from inside a runtime:
    /// the listener gets a thread and runtime of its own (see [`serve::spawn`]),
    /// which is what lets the three synchronous spawn seams start it without
    /// becoming `async` themselves.
    pub fn start(upstream_root: String) -> std::io::Result<Self> {
        let registry = Arc::new(Mutex::new(CredentialRegistry::default()));
        let port = serve::spawn(upstream_root, Arc::clone(&registry))?;
        Ok(Self { port, registry })
    }

    /// The loopback port this proxy accepted on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Find or mint the endpoint that forwards `upstream_key` to OpenRouter.
    ///
    /// Idempotent per key: calling it twice with the same key returns the same
    /// token, so repeated spawns do not accumulate registry entries.
    pub fn endpoint_for_key(&self, upstream_key: &str) -> ProxyEndpoint {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(token) = registry.by_key.get(upstream_key) {
            return ProxyEndpoint {
                port: self.port,
                token: token.clone(),
            };
        }
        // Reuses the login flow's nonce generator: 128 bits of OS-seeded entropy
        // with no `rand` dependency, which is the established precedent in this
        // crate for minting an unguessable local value.
        let token = format!("mdl-{}", crate::auth::random_state_nonce());
        registry
            .by_token
            .insert(token.clone(), upstream_key.to_string());
        registry
            .by_key
            .insert(upstream_key.to_string(), token.clone());
        ProxyEndpoint {
            port: self.port,
            token,
        }
    }
}

/// The process-wide proxy, started on first use.
///
/// A single listener serves every harness: the token, not the port, is what
/// separates credentials, so there is nothing to gain from a port per run and a
/// great deal of socket churn to lose.
static SHARED: std::sync::OnceLock<Result<ProxyHandle, String>> = std::sync::OnceLock::new();

/// The upstream root, honouring the [`UPSTREAM_URL_ENV`] test override.
fn upstream_root(env: &HashMap<String, String>) -> String {
    env.get(UPSTREAM_URL_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(OPENROUTER_ROOT)
        .to_string()
}

/// Return the shared proxy, starting it if this is the first caller.
///
/// The outcome is memoized either way. A bind failure is not retried on the next
/// spawn: it means the machine could not give this process a loopback socket, and
/// re-attempting once per harness launch would turn one clear error into a
/// stutter of identical ones.
pub fn shared(env: &HashMap<String, String>) -> Result<&'static ProxyHandle, String> {
    SHARED
        .get_or_init(|| {
            ProxyHandle::start(upstream_root(env))
                .map_err(|error| format!("could not start the local attribution proxy: {error}"))
        })
        .as_ref()
        .map_err(String::clone)
}

/// Whether `base_url` points at OpenRouter.
///
/// Host-based rather than prefix-based so a configured endpoint that differs
/// only in scheme, port or trailing path is still recognised, while a lookalike
/// domain (`openrouter.ai.example.com`) is not.
fn is_openrouter(base_url: &str) -> bool {
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

/// Rewrite `router` so every OpenRouter-bound provider reaches `proxy` instead.
///
/// Pure — no I/O, no environment. Returns `None` when no provider resolves to
/// OpenRouter, which is the "feature off" path: the caller keeps its original
/// config and behaviour is byte-for-byte what it was before this module existed.
///
/// Providers pointed somewhere *other* than OpenRouter are deliberately left
/// alone, but note the shared-credential consequence documented on
/// [`crate::config::RouterProviderConfig`]: `apiKeyEnv` is router-wide, so a
/// config mixing an OpenRouter provider with a non-OpenRouter one cannot give
/// them different keys. Such a mix is refused rather than silently handing the
/// loopback token to an endpoint that cannot honour it.
pub fn route_openrouter(
    router: &RouterConfig,
    proxy: &ProxyEndpoint,
    api_key_env: &str,
) -> Option<ProxyRouting> {
    let mut providers = router.providers.clone();
    let mut routed = false;
    let mut foreign = false;
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        let id = provider.as_str();
        let Some(base_url) = router.base_url_for(id) else {
            continue;
        };
        if !is_openrouter(base_url) {
            foreign = true;
            continue;
        }
        routed = true;
        let shape = UpstreamShape::for_provider(provider);
        providers.insert(
            id.to_string(),
            RouterProviderConfig {
                base_url: Some(proxy.base_url(shape)),
            },
        );
    }
    if !routed || foreign {
        return None;
    }

    // Both the configured name and the documented default are scrubbed: a preset
    // may reference a custom variable while the ambient `OPENROUTER_API_KEY` is
    // also exported, and leaving either behind reopens the bypass.
    let mut scrub_env = vec![OPENROUTER_API_KEY_ENV.to_string()];
    if !api_key_env.is_empty() && api_key_env != OPENROUTER_API_KEY_ENV {
        scrub_env.push(api_key_env.to_string());
    }

    Some(ProxyRouting {
        router: RouterConfig {
            // Cleared because every routed provider now carries an explicit
            // override; a stale top-level endpoint could otherwise be inherited
            // by a provider added later and quietly bypass the proxy.
            base_url: None,
            api_key_env: Some(PROXY_TOKEN_ENV.to_string()),
            models: router.models.clone(),
            providers,
        },
        env: vec![(PROXY_TOKEN_ENV.to_string(), proxy.token.clone())],
        scrub_env,
    })
}

/// Resolve the OpenRouter key `router` references, from a run environment.
///
/// Falls back to the documented default variable when the router names none, so
/// a `[router]` block that only sets `baseUrl` still routes through the proxy
/// rather than silently bypassing it. Returns the `(name, value)` pair because
/// the caller needs the name to scrub it.
fn resolve_key(router: &RouterConfig, env: &HashMap<String, String>) -> Option<(String, String)> {
    let name = router
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(OPENROUTER_API_KEY_ENV);
    let value = env.get(name).map(|value| value.trim())?;
    (!value.is_empty()).then(|| (name.to_string(), value.to_string()))
}

/// Route `router` through the shared proxy, starting it on first use.
///
/// The I/O-performing wrapper around [`route_openrouter`]. Returns `Ok(None)`
/// when the run is not OpenRouter-bound or its key is not exported — in both
/// cases the caller proceeds exactly as before, since a proxy with no upstream
/// credential could only turn a working run into a 401.
pub fn route_run(
    router: &RouterConfig,
    env: &HashMap<String, String>,
) -> Result<Option<ProxyRouting>, String> {
    let Some((name, key)) = resolve_key(router, env) else {
        return Ok(None);
    };
    // Cheap pre-check against a throwaway endpoint: starting a listener for a
    // config that turns out not to be OpenRouter-bound would be pure waste.
    let probe = ProxyEndpoint {
        port: 0,
        token: String::new(),
    };
    if route_openrouter(router, &probe, &name).is_none() {
        return Ok(None);
    }
    let handle = shared(env)?;
    let endpoint = handle.endpoint_for_key(&key);
    Ok(route_openrouter(router, &endpoint, &name))
}

/// Route one spawn's router and environment through the proxy, in place.
///
/// The single entry point every spawn seam calls. Keeping it one function is the
/// point: a seam that applied the router rewrite but forgot the environment
/// scrub would leave the real OpenRouter key in that seam's children, and the
/// harness could then bypass the proxy entirely — which is exactly the hole this
/// module exists to close.
///
/// A no-op for runs that are not OpenRouter-bound. Errors only when the proxy
/// could not be started, which is a hard error at the seam rather than a silent
/// direct spawn: falling back would send unattributed traffic and re-expose the
/// key, quietly undoing the thing the operator configured.
pub fn route_spawn(
    router: &mut Option<RouterConfig>,
    env: &mut HashMap<String, String>,
) -> Result<(), String> {
    let Some(configured) = router.as_ref() else {
        return Ok(());
    };
    let Some(routing) = route_run(configured, env)? else {
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
