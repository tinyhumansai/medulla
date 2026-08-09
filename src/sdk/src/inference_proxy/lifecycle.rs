//! Listener startup, shared ownership, and credential minting for the proxy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::serve;
use super::types::{
    CredentialRegistry, ProxyEndpoint, ProxyHandle, OPENROUTER_ROOT, UPSTREAM_URL_ENV,
};

impl ProxyHandle {
    /// Bind an ephemeral loopback port and start serving.
    ///
    /// `upstream_root` is the OpenRouter API root; tests pass a local mock so the
    /// suite stays offline. The listener owns a dedicated thread and runtime,
    /// making this synchronous entry point safe inside existing runtimes.
    pub fn start(upstream_root: String) -> std::io::Result<Self> {
        let registry = Arc::new(Mutex::new(CredentialRegistry::default()));
        let port = serve::spawn(upstream_root, Arc::clone(&registry))?;
        Ok(Self { port, registry })
    }

    /// Return the loopback port this proxy accepted on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Find or mint the endpoint that forwards `upstream_key` to OpenRouter.
    ///
    /// Token minting is idempotent per key, so repeated spawns do not grow the
    /// registry or expose distinct credentials for the same upstream secret.
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

        // Reuse the login flow's CSPRNG-backed nonce generator instead of
        // introducing a second randomness dependency for loopback tokens.
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
static SHARED: OnceLock<Result<ProxyHandle, String>> = OnceLock::new();

/// Resolve the proxy's upstream root, honoring the test-only environment override.
fn upstream_root(env: &HashMap<String, String>) -> String {
    env.get(UPSTREAM_URL_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(OPENROUTER_ROOT)
        .to_string()
}

/// Return the shared proxy, starting it if this is the first caller.
///
/// Startup outcomes are memoized. Repeatedly retrying a failed loopback bind at
/// every harness launch would obscure one actionable error with identical ones.
pub fn shared(env: &HashMap<String, String>) -> Result<&'static ProxyHandle, String> {
    SHARED
        .get_or_init(|| {
            ProxyHandle::start(upstream_root(env))
                .map_err(|error| format!("could not start the local attribution proxy: {error}"))
        })
        .as_ref()
        .map_err(String::clone)
}
