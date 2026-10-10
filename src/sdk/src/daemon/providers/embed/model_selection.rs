//! Model precedence: provider env override, generic env override, resolved node/preset/host model.

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
