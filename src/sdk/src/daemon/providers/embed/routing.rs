//! Resolve the caller-selected inference route through Medulla’s attribution proxy.

use std::collections::HashMap;

use crate::config::RouterConfig;
use crate::inference_proxy::EmbeddedRouting;

/// Resolve the route this turn should run on, if any.
///
/// `Ok(None)` means no per-call route could be built: a run with no router
/// preset at all, a router that names no endpoint, a router whose named key is
/// not exported, or a run with no model to route. The adapter refuses account-level fallback — the caller ([`super::run::run_local_task`]) treats a missing route
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
#[path = "routing_tests.rs"]
mod tests;
