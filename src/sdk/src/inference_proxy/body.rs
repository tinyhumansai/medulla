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

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(body: &str, pins: &[&str]) -> Option<serde_json::Value> {
        let pins: Vec<String> = pins.iter().map(|slug| (*slug).to_string()).collect();
        inject_provider_only(&Bytes::from(body.to_string()), &pins)
            .map(|out| serde_json::from_slice(&out).expect("rewritten body is JSON"))
    }

    #[test]
    fn adds_provider_only_and_keeps_the_request_intact() {
        let out = rewrite(
            r#"{"model":"z-ai/glm-5.2","max_tokens":16}"#,
            &["streamlake"],
        )
        .expect("pinned body is rewritten");
        assert_eq!(out["provider"]["only"], serde_json::json!(["streamlake"]));
        assert_eq!(out["model"], "z-ai/glm-5.2");
        assert_eq!(out["max_tokens"], 16);
    }

    #[test]
    fn preserves_sibling_provider_keys() {
        let out = rewrite(
            r#"{"model":"m","provider":{"allow_fallbacks":false}}"#,
            &["streamlake"],
        )
        .expect("pinned body is rewritten");
        assert_eq!(out["provider"]["allow_fallbacks"], false);
        assert_eq!(out["provider"]["only"], serde_json::json!(["streamlake"]));
    }

    #[test]
    fn keeps_every_named_provider_in_order() {
        let out = rewrite(r#"{"model":"m"}"#, &["streamlake", "novita"])
            .expect("pinned body is rewritten");
        assert_eq!(
            out["provider"]["only"],
            serde_json::json!(["streamlake", "novita"])
        );
    }

    #[test]
    fn forwards_unchanged_without_a_pin() {
        assert!(rewrite(r#"{"model":"m"}"#, &[]).is_none());
    }

    #[test]
    fn does_not_override_a_caller_s_own_restriction() {
        assert!(rewrite(
            r#"{"model":"m","provider":{"only":["novita"]}}"#,
            &["streamlake"]
        )
        .is_none());
    }

    #[test]
    fn forwards_unchanged_when_the_body_is_not_a_json_object() {
        assert!(rewrite("not json at all", &["streamlake"]).is_none());
        assert!(rewrite("[1,2,3]", &["streamlake"]).is_none());
    }

    #[test]
    fn forwards_unchanged_when_provider_is_not_an_object() {
        assert!(rewrite(r#"{"model":"m","provider":"streamlake"}"#, &["streamlake"]).is_none());
    }
}
