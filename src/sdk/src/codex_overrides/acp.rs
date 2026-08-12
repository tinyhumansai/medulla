//! Delivering the routed-Codex overrides over the ACP transport.
//!
//! The CLI seam hands Codex its overrides as `-c key=value` argv. `codex-acp`
//! ignores argv entirely once it is in server mode: it reads its configuration
//! from two environment variables — `CODEX_CONFIG`, a JSON document merged into
//! every session's config, and `MODEL_PROVIDER`, the provider id to select — and
//! then starts `codex` itself over the app-server protocol.
//!
//! So argv is not a second way to say the same thing here; it is silently
//! discarded. A routed ACP run that only passed `-c` reached Codex with no
//! provider block at all, which does not fail — it succeeds against the
//! operator's own account and default model, with the preset's endpoint sitting
//! unused in the environment. That is the quietest possible way for a preset to
//! be wrong, and it is what this module exists to prevent.
//!
//! The overrides themselves are [`super::overrides`], shared with the CLI seam,
//! so the two transports cannot disagree about where a routed run points.

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::types::CodexOverridesError;
use super::PROVIDER_ID;
use crate::protocol::HarnessProvider;

/// The JSON config document `codex-acp` merges into every session it opens.
pub const CONFIG_ENV: &str = "CODEX_CONFIG";
/// The provider id `codex-acp` selects for a session.
pub const MODEL_PROVIDER_ENV: &str = "MODEL_PROVIDER";

/// The environment a routed Codex ACP spawn needs, or empty when this does not
/// apply (see [`super::overrides`] for the three cases that yield nothing).
///
/// # Errors
///
/// Propagates a catalog that cannot be derived, for the same reason the CLI seam
/// does: reaching the provider with tool shapes it rejects is a 400 on the first
/// turn, far harder to read than a message naming the cache file.
pub fn acp_env(
    provider: HarnessProvider,
    model: Option<&str>,
    env: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, CodexOverridesError> {
    let overrides = super::overrides(provider, model, env)?;
    if overrides.is_empty() {
        return Ok(Vec::new());
    }
    let mut document = Map::new();
    for (key, value) in &overrides {
        insert_dotted(&mut document, key, Value::String(value.clone()));
    }
    // The CLI seam selects the model with `-m`; ACP has no argv to put it on, so
    // the model is part of the config here or it is not selected at all. Without
    // it `codex-acp` opens the session on Codex's own default model — which the
    // routed catalog does not describe, so the run either fails or bills a model
    // the preset never asked for.
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        document.insert("model".to_string(), Value::String(model.to_string()));
    }
    let encoded = serde_json::to_string(&Value::Object(document))
        .map_err(|source| CodexOverridesError::Encode { source })?;
    Ok(vec![
        (CONFIG_ENV.to_string(), encoded),
        (MODEL_PROVIDER_ENV.to_string(), PROVIDER_ID.to_string()),
    ])
}

/// Insert `value` at a dotted `key`, creating intermediate objects.
///
/// `model_providers.medulla.base_url` is one key in the CLI's flat `-c` syntax
/// and three nested objects in JSON. A segment already holding a non-object is
/// overwritten rather than merged into: the keys come from
/// [`super::overrides`], which never emits a prefix and a longer path that
/// disagree, so a collision would mean a bug here rather than operator input to
/// preserve.
fn insert_dotted(document: &mut Map<String, Value>, key: &str, value: Value) {
    let mut segments = key.split('.').peekable();
    let mut cursor = document;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            cursor.insert(segment.to_string(), value);
            return;
        }
        let entry = cursor
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cursor = entry
            .as_object_mut()
            .expect("the entry was just made an object");
    }
}
