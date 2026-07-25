//! Tests for the transport module.

use super::*;

#[test]
fn session_error_classifier() {
    assert!(is_session_error("MAC verification failed during decrypt"));
    assert!(is_session_error("No session for ABC"));
    assert!(is_session_error("Signed pre-key spk_1 not found"));
    assert!(!is_session_error("not_a_contact"));
    assert!(!is_session_error("HTTP 500"));
}

#[test]
fn decode_agent_id_rejects_non_32_bytes() {
    assert!(decode_agent_id("!!!not-base58!!!").is_err());
}

#[test]
fn decode_agent_id_accepts_a_real_32_byte_key() {
    // A freshly generated signer's agent id is a base58 32-byte Ed25519 key.
    let signer = LocalSigner::generate();
    let decoded = decode_agent_id(&signer.agent_id()).expect("valid agent id");
    assert_eq!(decoded.len(), 32);
}

#[test]
fn session_error_classifier_matches_prekey_variants() {
    assert!(is_session_error("Key bundle rejected: bad signed pre-key"));
    assert!(is_session_error("prekey missing"));
    assert!(is_session_error("no session established"));
}
