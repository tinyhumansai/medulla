//! Which model an embedded OpenHuman turn runs on.
//!
//! Every route an operator has for choosing that model converges here, so the
//! precedence between them is stated in one place and testable without booting
//! the core.
//!
//! # The routes, and where each one is decided
//!
//! By the time [`run_openhuman_task`](super::run::run_openhuman_task) is
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
//! 6. nothing — the core's own `default_model` answers, as it always did
//!
//! The environment sits on top for the same reason `MEDULLA_<P>_BIN` does: it
//! is the operator's override of what the configuration says, applied to the
//! machine they are standing at. Steps 3–5 are resolved upstream and arrive as
//! one `Option<String>`.
//!
//! # What choosing a model here does not do
//!
//! The name is handed to the core as `model_override` and the core resolves it
//! against *its own* provider bindings. A model id no configured provider
//! serves is not an error here: the core's agent loop falls through to its
//! resolved default and emits its own "override skipped" diagnostic. So a
//! preset's `baseUrl` and `apiKeyEnv` are inert for this provider — the
//! embedded core owns its credentials and endpoints, and Medulla injects
//! neither into a turn that spawns no child.

use std::collections::HashMap;

use crate::protocol::HarnessProvider;

/// The model an embedded turn should ask the core for.
///
/// `requested` is what the dispatch already resolved (node, preset, host
/// default); `env` is the run's environment. See the module docs for the full
/// precedence order and for why the environment outranks `requested`.
///
/// Blank values on either side are treated as unset, so an empty
/// `--model ""` or an exported-but-empty variable falls through to the next
/// route rather than asking the core for the empty model.
pub fn effective_model(requested: Option<String>, env: &HashMap<String, String>) -> Option<String> {
    crate::protocol::env::model_override(HarnessProvider::Openhuman, env).or_else(|| {
        requested
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
    })
}
