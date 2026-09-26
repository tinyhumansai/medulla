//! Unit tests for the request-body rewrite that applies an upstream-provider
//! pin.
//!
//! The rewrite is a pure function: given a pinned request body and its
//! `provider_only` list, it either marks the body with `provider.only` or
//! returns `None` to forward it unchanged. The streaming forward trusts that
//! result wholesale, so each branch — a caller that already restricted itself, a
//! body that is not a JSON object, an empty pin, sibling provider keys — is
//! pinned down here rather than left to conversation.

use bytes::Bytes;

use super::body::inject_provider_only;

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
    let out =
        rewrite(r#"{"model":"m"}"#, &["streamlake", "novita"]).expect("pinned body is rewritten");
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
