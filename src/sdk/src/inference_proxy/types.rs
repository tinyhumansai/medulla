//! Data types for the loopback inference proxy: the wire shape a harness speaks,
//! the loopback endpoint a child is pointed at, and the router rewrite handed to
//! a spawn seam.
//!
//! Only shapes and their trivial impls live here. Minting tokens is
//! [`super::lifecycle`]'s job, key resolution is [`super::routing`]'s, and
//! forwarding is [`super::serve`]'s.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::RouterConfig;
use crate::protocol::HarnessProvider;

/// The child environment variable naming the loopback token.
///
/// This is a *name*, matching the repo-wide convention that config carries the
/// name of an env var and the spawn seam resolves its value. The existing
/// `RouterInjection::secret_env` machinery therefore needs no change: it looks
/// this name up in the run environment exactly as it would look up
/// `OPENROUTER_API_KEY`.
pub const PROXY_TOKEN_ENV: &str = "MEDULLA_PROXY_TOKEN";

/// Overrides the upstream the proxy forwards to. Tests point this at a local
/// mock so the suite stays offline; operators should not normally set it.
pub const UPSTREAM_URL_ENV: &str = "MEDULLA_OPENROUTER_URL";

/// OpenRouter's API root — the Anthropic-shaped base, with the OpenAI-shaped one
/// a `/v1` below it. Mirrors the constants in
/// [`crate::config::custom_harnesses`], which describe the same two endpoints
/// from the configuration side.
pub const OPENROUTER_ROOT: &str = "https://openrouter.ai/api";

/// What one loopback token authorizes: an upstream credential, plus the
/// upstream-provider restriction the run asked for.
///
/// The pin rides with the credential rather than being looked up per request
/// because the token is the only thing a forwarded request carries — the proxy
/// has no other handle on which preset produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct UpstreamCredential {
    /// The real OpenRouter key this token stands in for.
    pub(super) key: String,
    /// OpenRouter provider slugs the request is restricted to. Empty forwards
    /// the body untouched.
    pub(super) provider_only: Vec<String>,
}

impl UpstreamCredential {
    /// The registry identity for this credential.
    ///
    /// Two presets sharing an `apiKeyEnv` but pinning different providers are
    /// *different* upstreams and must not share a token: reusing one would let
    /// whichever preset minted it first decide where the other's traffic is
    /// served. The key is included so the identity stays unique per credential,
    /// and `\u{0}` separates the parts because neither a key nor a provider slug
    /// can contain it.
    pub(super) fn identity(&self) -> String {
        let mut identity = self.key.clone();
        for slug in &self.provider_only {
            identity.push('\u{0}');
            identity.push_str(slug);
        }
        identity
    }
}

/// The loopback token to upstream-credential mapping used by the serving loop.
///
/// Keyed both ways so request authentication is fast while token minting stays
/// idempotent for repeated spawns using the same OpenRouter credential and pin.
#[derive(Debug, Default)]
pub(super) struct CredentialRegistry {
    pub(super) by_token: HashMap<String, UpstreamCredential>,
    pub(super) by_identity: HashMap<String, String>,
}

/// A running proxy and the credentials it accepts.
///
/// One handle serves every harness in the process. Clones share the credential
/// registry, so a token minted through one clone is immediately usable by the
/// serving loop.
#[derive(Debug, Clone)]
pub struct ProxyHandle {
    pub(super) port: u16,
    pub(super) registry: Arc<Mutex<CredentialRegistry>>,
}

/// The request dialect a harness speaks, which decides both the mount it is
/// pointed at and the upstream path the mount maps onto.
///
/// This is not the same axis as [`HarnessProvider`]: Codex and OpenCode are
/// different harnesses speaking one dialect, and a future harness would pick a
/// dialect rather than add a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamShape {
    /// Anthropic Messages wire format, reached via `ANTHROPIC_BASE_URL`.
    Anthropic,
    /// OpenAI Chat Completions wire format, reached via `OPENAI_BASE_URL`.
    OpenAi,
}

impl UpstreamShape {
    /// The dialect a harness speaks when routed.
    ///
    /// Matches [`crate::protocol::env::router_env`]'s own provider split: Claude
    /// Code is given an `ANTHROPIC_BASE_URL`, Codex and OpenCode an
    /// `OPENAI_BASE_URL`. Keeping the two in agreement is what makes a mount
    /// reachable at all.
    pub fn for_provider(provider: HarnessProvider) -> Self {
        match provider {
            HarnessProvider::Claude => Self::Anthropic,
            HarnessProvider::Codex
            | HarnessProvider::Opencode
            | HarnessProvider::Openhuman
            // Never routed — a shell is not given a router at all — but it has
            // to name a dialect, and the OpenAI-shaped one is the default the
            // rest of the non-Anthropic providers share.
            | HarnessProvider::Shell => Self::OpenAi,
        }
    }

    /// The first path segment identifying this dialect's mount.
    pub fn mount(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    /// The upstream base this mount forwards onto, below `root`.
    ///
    /// The two differ by a `/v1`: an `OPENAI_BASE_URL` conventionally already
    /// includes the version segment (clients request `/chat/completions` under
    /// it), while an Anthropic client appends its own (`/v1/messages`).
    pub fn upstream_base(self, root: &str) -> String {
        let root = root.trim_end_matches('/');
        match self {
            Self::Anthropic => root.to_string(),
            Self::OpenAi => format!("{root}/v1"),
        }
    }

    /// Every dialect, for callers that must cover all mounts.
    pub const ALL: [Self; 2] = [Self::Anthropic, Self::OpenAi];
}

/// Where one credential's traffic enters the proxy.
///
/// A distinct endpoint exists per upstream credential, not per run: the token is
/// what the serving loop maps back to an OpenRouter key, so two presets sharing
/// an `apiKeyEnv` share a token and two presets with different keys do not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEndpoint {
    /// The loopback port the proxy accepted on.
    pub port: u16,
    /// The bearer token the child presents. Local-only: it authenticates against
    /// this process and is worthless anywhere else.
    pub token: String,
}

impl ProxyEndpoint {
    /// The base URL a child harness speaking `shape` should be pointed at.
    ///
    /// Bound to `127.0.0.1` rather than `localhost` so resolution cannot be
    /// redirected by a hosts file or land on an IPv6 loopback the listener is
    /// not bound to.
    pub fn base_url(&self, shape: UpstreamShape) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, shape.mount())
    }
}

/// Where an **in-process** caller sends its inference traffic.
///
/// The counterpart to [`ProxyRouting`] for a run with no child. The embedded
/// OpenHuman core is configured with these two values directly, so there is no
/// router document to rewrite and nothing to scrub.
///
/// [`route_embedded`](super::route_embedded) fills it one of two ways. For an
/// OpenRouter endpoint these are the proxy's loopback mount and its
/// machine-local token, so the real key never enters a process environment on
/// this path. For any other endpoint they are the operator's own endpoint and
/// key, unproxied — there is no third party to credit and no header to rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedRouting {
    /// The OpenAI-compatible base URL to point the core at: the proxy's
    /// loopback mount, or the configured endpoint itself.
    pub base_url: String,
    /// The credential the core presents to `base_url` — the loopback token for
    /// a proxied route, the configured key for a direct one.
    pub token: String,
}

/// The rewrite a spawn seam applies so its child reaches OpenRouter through the
/// proxy instead of directly.
///
/// Produced by [`super::route_openrouter`] as a pure value: applying it is the
/// caller's job, which keeps the three spawn seams' differing env plumbing out
/// of this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRouting {
    /// The router config to use in place of the original — same shape, endpoints
    /// swapped for loopback mounts and the key name swapped for the token name.
    pub router: RouterConfig,
    /// Environment entries the child needs, i.e. the token under
    /// [`PROXY_TOKEN_ENV`].
    pub env: Vec<(String, String)>,
    /// Environment variable *names* to remove from the child.
    ///
    /// The real OpenRouter key must not survive anywhere in the child's
    /// environment: leaving it would let a harness bypass the proxy entirely and
    /// reach OpenRouter unattributed, which is the whole problem this module
    /// exists to solve.
    pub scrub_env: Vec<String>,
}
