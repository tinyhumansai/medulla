//! Where an embedded OpenHuman turn's inference goes.
//!
//! The sibling of [`super::model`]: that module answers *which model*, this one
//! answers *whose endpoint and whose credential*. They are separate questions
//! because a model name alone is inert — the core resolves it against its own
//! provider bindings, and a name no configured provider serves falls back to the
//! account's default with a diagnostic.
//!
//! # What changed, and why this module exists
//!
//! A `[[customHarnesses]]` preset carries `model`, `baseUrl` and `apiKeyEnv`.
//! For a spawned CLI all three are live: the endpoint and key are layered into
//! the child's environment at the spawn seam. For an OpenHuman turn there is no
//! child, so the latter two used to be documented as inert — the preset could
//! name a model but not the provider that serves it, and the operator had to
//! configure the core separately for the preset to do anything.
//!
//! It no longer is. The core accepts a per-call route (`inference_url` +
//! `api_key` on `inference_agent_chat`) that it applies to the turn's own
//! in-memory config and never persists, so a preset can state the whole answer:
//! this model, on this endpoint, with this key.
//!
//! # Why the core is not given the OpenRouter key
//!
//! For the same reason a child harness is not — see [`crate::inference_proxy`].
//! The endpoint handed over is Medulla's loopback mount and the credential is a
//! machine-local token, so the traffic is re-headed on the way out and credited
//! to Medulla rather than to OpenHuman. The one difference from a spawned run is
//! that there is no environment to scrub: the key is read here, exchanged for a
//! token here, and never written anywhere the turn can reach it.
//!
//! # Endpoints that are not OpenRouter
//!
//! A preset may name a local router, a self-hosted gateway, or a vendor's own
//! OpenAI-compatible endpoint. Those are handed to the core as they are spelled,
//! with the key the preset named: there is no third party to credit and no
//! header to rewrite, so interposing the attribution proxy would add a hop that
//! rewrites requests for an upstream it knows nothing about. That decision is
//! [`crate::inference_proxy::route_embedded`]'s; this module only supplies the
//! preconditions.

use std::collections::HashMap;

use crate::config::RouterConfig;
use crate::inference_proxy::EmbeddedRouting;

/// Resolve the route this turn should run on, if any.
///
/// `Ok(None)` — the common case — means the turn runs on whatever the account's
/// own OpenHuman configuration resolves, which is the behaviour every run had
/// before presets could state an endpoint. It covers a run with no router at
/// all, a router that names no endpoint, a router whose named key is not
/// exported, and a run with no model to route.
///
/// The model is required because the core's per-call route is expressed as a
/// provider/model pair: with no model there is nothing to pin the route to, and
/// the core would ignore it. Reporting that here as "not routed" keeps the two
/// sides agreeing rather than sending a request the core discards.
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
