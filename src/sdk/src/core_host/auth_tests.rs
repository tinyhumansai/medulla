//! Unit tests for the auth helpers' pure parts.
//!
//! The RPC calls themselves need a booted core (process globals, singleton event
//! bus) and belong in an integration test. What is testable here is the response
//! handling — the log-envelope unwrap and the auth-state decode — which is where
//! a silent misread would turn a signed-in operator into a signed-out one.

use super::auth::{unwrap_envelope, AuthState};
use serde_json::json;

#[test]
fn a_logging_handlers_envelope_is_unwrapped() {
    // Whether a method logs is an implementation detail of its handler; a
    // caller that decoded the raw value would break the day one is added.
    let raw = json!({ "result": { "token": "jwt-1" }, "logs": ["session token fetched"] });
    assert_eq!(unwrap_envelope(raw), json!({ "token": "jwt-1" }));
}

#[test]
fn a_silent_handlers_payload_passes_through() {
    let raw = json!({ "token": "jwt-1" });
    assert_eq!(unwrap_envelope(raw.clone()), raw);
}

#[test]
fn a_payload_that_merely_has_a_result_field_is_not_unwrapped() {
    // The strict shape check is the point: unwrapping this would hand the
    // caller the inner value and lose every sibling field.
    let raw = json!({ "result": "ok", "other": 1 });
    assert_eq!(unwrap_envelope(raw.clone()), raw);
}

#[test]
fn an_envelope_with_non_array_logs_is_not_unwrapped() {
    let raw = json!({ "result": "ok", "logs": "one line" });
    assert_eq!(unwrap_envelope(raw.clone()), raw);
}

#[test]
fn auth_state_decodes_the_signed_in_case() {
    let state: AuthState =
        serde_json::from_value(json!({ "isAuthenticated": true, "userId": "u-1" })).unwrap();
    assert!(state.is_authenticated);
    assert_eq!(state.user_id.as_deref(), Some("u-1"));
}

#[test]
fn auth_state_tolerates_fields_this_host_does_not_model() {
    // The core's response carries more than this host reads. An addition
    // upstream must not fail the decode and read as "signed out".
    let state: AuthState = serde_json::from_value(json!({
        "isAuthenticated": true,
        "userId": "u-1",
        "profileId": "app-session:default",
        "user": { "email": "someone@example.test" }
    }))
    .unwrap();
    assert!(state.is_authenticated);
}

#[test]
fn auth_state_defaults_to_signed_out() {
    let state: AuthState = serde_json::from_value(json!({})).unwrap();
    assert!(!state.is_authenticated);
    assert!(state.user_id.is_none());
}
