//! A loopback HTTP proxy that owns Medulla's OpenRouter attribution.
//!
//! # Why this exists
//!
//! When an operator supplies an OpenRouter key, Medulla points the harness at
//! OpenRouter through [`crate::config::RouterConfig`] and
//! [`crate::protocol::env::router_env`]. That works, but it hands OpenRouter a
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
//! - [`body`] — the request-body rewrite that applies an upstream-provider pin.
//! - [`headers`] — the pure request-header rewrite. All attribution policy.
//! - [`lifecycle`] — listener startup and process-wide proxy ownership.
//! - [`routing`] — provider-scoped router and child-environment rewriting.
//! - [`serve`] — the accept loop and the streaming forward.
//! - [`types`] — endpoint, dialect, and the router rewrite handed to a seam.
//!
//! # Known limitation
//!
//! This repo's boundary is env injection at the spawn seam; Medulla never writes
//! a harness's on-disk configuration. Attribution is therefore guaranteed for
//! Medulla-routed runs only. A harness the operator has separately configured to
//! reach OpenRouter through its own config file still bypasses this proxy.

mod body;
mod headers;
mod lifecycle;
mod routing;
mod serve;
mod types;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod body_tests;

pub use body::MAX_REWRITE_BYTES;
pub use headers::{rewrite, MEDULLA_REFERER, MEDULLA_TITLE};
pub use lifecycle::shared;
pub use routing::{route_openrouter, route_run, route_spawn};
pub use types::{
    ProxyEndpoint, ProxyHandle, ProxyRouting, UpstreamShape, OPENROUTER_ROOT, PROXY_TOKEN_ENV,
    UPSTREAM_URL_ENV,
};
