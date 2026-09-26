//! Which model a local (in-process OpenHuman) turn runs on.
//!
//! Every route an operator has for choosing that model converges here, so the
//! precedence between them is stated in one place and testable without a live
//! inference route.
//!
//! # The routes, and where each one is decided
//!
//! By the time [`run_local_task`](super::run::run_local_task) is
//! called, most of them have already collapsed into
//! [`RunTaskOptions::model`](crate::daemon::providers::RunTaskOptions::model) —
//! the workflow dispatch resolves the node's own `config.model`, then the
//! selected `[[customHarnesses]]` preset's `model`, then the host's default
//! (`[workflows] defaultModel`, or `medulla workflow run --model` for one
//! invocation). What has *not* been consulted at that point is the process
//! environment, which is this module's job.
//!
//! Effective order, highest first:
//!
//! 1. `MEDULLA_OPENHUMAN_MODEL` (deprecated spelling: `TINYPLACE_OPENHUMAN_MODEL`)
//! 2. `MEDULLA_HARNESS_MODEL` (deprecated spelling: `TINYPLACE_HARNESS_MODEL`)
//! 3. the node's / task request's own model
//! 4. the selected custom harness preset's `model`
//! 5. `medulla workflow run --model <name>`, then `[workflows] defaultModel`
//! 6. nothing — unlike a spawned CLI provider, which can pick its own default,
//!    this harness has none to fall back to: the run is refused with a
//!    diagnostic rather than started with an unset model
//!
//! The environment sits on top for the same reason `MEDULLA_<P>_BIN` does: it
//! is the operator's override of what the configuration says, applied to the
//! machine they are standing at. Steps 3–5 are resolved upstream and arrive as
//! one `Option<String>`.
//!
//! # What choosing a model here does not do
//!
//! Naming a model here is only half the answer: on its own it says nothing
//! about which endpoint or credential should serve it, and this harness has no
//! account-level configuration to fall back to if the pair is left unresolved.
//! A model id the resolved endpoint does not recognize is not caught here — it
//! surfaces as whatever error that endpoint returns.
//!
//! What makes a named model actually reachable is the endpoint and credential
//! beside it, which is [`super::router`]'s job. A preset that carries a
//! `baseUrl` and an `apiKeyEnv` states the whole answer and the turn runs there;
//! a preset that carries only a model has nothing to route on, and
//! [`super::router::embedded_route`] reports the run as not routed.

use std::collections::HashMap;

use crate::protocol::HarnessProvider;

/// The model a local turn's route should be pinned to.
///
/// `requested` is what the dispatch already resolved (node, preset, host
/// default); `env` is the run's environment. See the module docs for the full
/// precedence order and for why the environment outranks `requested`.
///
/// Blank values on either side are treated as unset, so an empty
/// `--model ""` or an exported-but-empty variable falls through to the next
/// route rather than resolving to the empty model.
pub fn effective_model(requested: Option<String>, env: &HashMap<String, String>) -> Option<String> {
    crate::protocol::env::model_override(HarnessProvider::Openhuman, env).or_else(|| {
        requested
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
    })
}
