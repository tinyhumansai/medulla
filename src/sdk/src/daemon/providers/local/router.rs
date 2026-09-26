//! Where a local (in-process OpenHuman) turn's inference goes.
//!
//! The sibling of [`super::model`]: that module answers *which model*, this one
//! answers *whose endpoint and whose credential*. They are separate questions
//! because a model name alone is inert — it has to be paired with an endpoint
//! and a credential that actually serves it, or the call fails with a
//! diagnostic.
//!
//! # What this module resolves
//!
//! A `[[customHarnesses]]` preset carries `model`, `baseUrl` and `apiKeyEnv`.
//! For a spawned CLI all three are live: the endpoint and key are layered into
//! the child's environment at the spawn seam. For a local turn there is no
//! child process to hand an environment to, so this module builds the
//! equivalent as a per-call [`Route`](crate::agent::harness::Route) instead:
//! this model, on this endpoint, with this key, passed straight into the
//! harness call rather than persisted anywhere.
//!
//! # Why the OpenRouter key is not forwarded as-is
//!
//! For the same reason a child harness does not get it directly — see
//! [`crate::inference_proxy`]. The endpoint handed to the turn is Medulla's
//! loopback mount and the credential is a machine-local token, so the traffic
//! is re-headed on the way out and credited to Medulla rather than to
//! OpenHuman. The one difference from a spawned run is that there is no
//! environment to scrub: the key is read here, exchanged for a token here, and
//! never written anywhere the turn's own process could leak it.
//!
//! # Endpoints that are not OpenRouter
//!
//! A preset may name a local router, a self-hosted gateway, or a vendor's own
//! OpenAI-compatible endpoint. Those reach the turn exactly as spelled, with the
//! key the preset named: there is no third party to credit and no header to
//! rewrite, so interposing the attribution proxy would add a hop that rewrites
//! requests for an upstream it knows nothing about. That decision is
//! [`crate::inference_proxy::route_embedded`]'s; this module only supplies the
//! preconditions.

use std::collections::HashMap;

use crate::config::RouterConfig;
use crate::inference_proxy::EmbeddedRouting;

/// Resolve the route this turn should run on, if any.
///
/// `Ok(None)` means no per-call route could be built: a run with no router
/// preset at all, a router that names no endpoint, a router whose named key is
/// not exported, or a run with no model to route. Unlike the embedded core this
/// harness replaced, there is no account-level inference configuration to fall
/// back to — the caller ([`super::run::run_local_task`]) treats a missing route
/// as a hard error rather than a silent default, because there is nothing left
/// to run the turn on.
///
/// The model is required because the resolved route is expressed as a
/// provider/model pair: with no model there is nothing to pin the route to.
/// Reporting that here as "not routed" keeps this module's answer and the
/// caller's requirement agreeing, rather than building a route the caller
/// rejects anyway.
///
/// # Errors
///
/// The sentence [`crate::inference_proxy::route_embedded`] produces when the
/// loopback listener cannot bind. Fatal on purpose: the operator asked for a
/// specific endpoint, and quietly running the turn on a different provider —
/// billed to a different account — is worse than failing.
pub fn embedded_route(
    router: Option<&RouterConfig>,
    env: &HashMap<String, String>,
    model: Option<&str>,
) -> Result<Option<EmbeddedRouting>, String> {
    let Some(router) = router else {
        return Ok(None);
    };
    if model.map(str::trim).is_none_or(str::is_empty) {
        tracing::debug!(
            "[openhuman] not routing this turn: an endpoint was configured but no model was resolved"
        );
        return Ok(None);
    }
    let route = crate::inference_proxy::route_embedded(router, env)?;
    if route.is_some() {
        tracing::debug!("[openhuman] routing this turn to the endpoint the preset named");
    }
    Ok(route)
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
