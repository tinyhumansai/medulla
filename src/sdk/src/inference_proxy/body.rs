//! Request-body rewriting for upstream-provider pinning.
//!
//! OpenRouter chooses which of a model's serving providers answers a request,
//! and the same model is offered at prices that differ by more than an order of
//! magnitude. The only way to state that choice is the request body's
//! `provider` object: it is not a header, not a query parameter, and not a
//! model-id suffix — an unrecognized suffix such as `z-ai/glm-5.2:streamlake`
//! is accepted and silently ignored, which is worse than an error.
//!
//! Body rewriting is therefore the only seam available, and it is confined to
//! this module so [`super::serve`] keeps its streaming forward for every run
//! that has not asked for a pin.

use bytes::Bytes;

/// The largest request body this module will buffer in order to rewrite it.
///
/// A rewrite has to hold the whole body to parse it, which is exactly what the
/// streaming forward exists to avoid. 32 MiB clears any realistic prompt — a
/// million-token context serializes well under it — while bounding what a
/// single request can pin in memory. A pinned request whose body exceeds this
/// is forwarded without the pin rather than buffered (see
/// [`crate::inference_proxy::serve`]).
pub const MAX_REWRITE_BYTES: usize = 32 * 1024 * 1024;

/// Merge `provider.only` into a JSON request body.
///
/// Returns `None` when the body should be forwarded unchanged: no pin
/// configured, a body that is not a JSON object (streamed uploads, malformed
/// requests), or one already naming its own `only` — a caller that has stated a
/// restriction is not second-guessed.
///
/// Sibling keys under `provider` are preserved, so a body carrying, say, its own
/// `allow_fallbacks` keeps it.
pub(super) fn inject_provider_only(body: &Bytes, provider_only: &[String]) -> Option<Bytes> {
    if provider_only.is_empty() {
        return None;
    }
    let mut document: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;

    let provider = object
        .entry("provider")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let provider = provider.as_object_mut()?;
    if provider.contains_key("only") {
        return None;
    }
    provider.insert(
        "only".to_string(),
        serde_json::Value::Array(
            provider_only
                .iter()
                .map(|slug| serde_json::Value::String(slug.clone()))
                .collect(),
        ),
    );

    serde_json::to_vec(&document).ok().map(Bytes::from)
}
