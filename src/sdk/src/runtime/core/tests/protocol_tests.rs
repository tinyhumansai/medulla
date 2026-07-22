//! Frame-grammar tests: parsing each serve→host NDJSON frame kind, the
//! `ready` handshake classification, and the host→serve `req`/`ret`/`hello`
//! serialization (pure functions in `super::super::protocol`, no sockets).

use serde_json::{json, Value};

use super::super::protocol::{
    check_ready, hello_params, parse_line, port_unavailable_ret, req_line, Inbound, ReadyCheck,
};
use super::super::types::PROTOCOL_VERSION;

#[test]
fn parse_line_decodes_each_serve_to_host_frame() {
    match parse_line(
        r#"{"t":"ready","protocol":1,"serve":"3.12.0","sessionId":"agent","error":null}"#,
    ) {
        Some(Inbound::Ready {
            protocol,
            serve,
            session_id,
            error,
        }) => {
            assert_eq!(protocol, 1);
            assert_eq!(serve.as_deref(), Some("3.12.0"));
            assert_eq!(session_id.as_deref(), Some("agent"));
            assert!(error.is_none());
        }
        other => panic!("expected Ready, got {other:?}"),
    }
    match parse_line(r#"{"t":"res","id":"h1","ok":true,"result":{"x":1}}"#) {
        Some(Inbound::Res {
            id,
            ok,
            result,
            error,
        }) => {
            assert_eq!(id, "h1");
            assert!(ok);
            assert_eq!(result, json!({"x":1}));
            assert!(error.is_none());
        }
        other => panic!("expected Res, got {other:?}"),
    }
    match parse_line(
        r#"{"t":"res","id":"2","ok":false,"error":{"code":"timeout","message":"slow"}}"#,
    ) {
        Some(Inbound::Res { ok, error, .. }) => {
            assert!(!ok);
            let e = error.unwrap();
            assert_eq!(e.code, "timeout");
            assert_eq!(e.message, "slow");
        }
        other => panic!("expected failed Res, got {other:?}"),
    }
    match parse_line(r#"{"t":"call","id":"c1","port":"inference","method":"invoke"}"#) {
        Some(Inbound::Call { id, port }) => {
            assert_eq!(id, "c1");
            assert_eq!(port, "inference");
        }
        other => panic!("expected Call, got {other:?}"),
    }
    match parse_line(
        r#"{"t":"event","seq":7,"at":0,"event":{"kind":"cycle_start","instructionId":"i","cycleId":"c"}}"#,
    ) {
        Some(Inbound::Event { seq, event }) => {
            assert_eq!(seq, 7);
            assert_eq!(event.get("kind").unwrap(), "cycle_start");
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn parse_line_skips_malformed_and_unknown() {
    assert!(parse_line("").is_none());
    assert!(parse_line("   ").is_none());
    assert!(parse_line("not json").is_none());
    assert!(parse_line(r#"{"no":"discriminant"}"#).is_none());
    assert!(parse_line(r#"{"t":"emit","id":"c1"}"#).is_none()); // host→serve, never inbound
    assert!(parse_line(r#"{"t":"res"}"#).is_none()); // missing id
}

#[test]
fn check_ready_flags_mismatch_and_startup_error() {
    assert!(matches!(
        check_ready(
            PROTOCOL_VERSION,
            Some("v".into()),
            Some("agent".into()),
            None
        ),
        ReadyCheck::Ok { .. }
    ));
    assert!(matches!(
        check_ready(2, None, None, None),
        ReadyCheck::Fatal(_)
    ));
    assert!(matches!(
        check_ready(1, None, None, Some("boom".into())),
        ReadyCheck::Fatal(_)
    ));
}

#[test]
fn outbound_frames_are_well_formed() {
    let req = req_line("r1", "instruct", &json!({"message":"hi"}));
    assert!(req.ends_with('\n'));
    let v: Value = serde_json::from_str(req.trim()).unwrap();
    assert_eq!(v["t"], "req");
    assert_eq!(v["op"], "instruct");
    assert_eq!(v["id"], "r1");

    let ret = port_unavailable_ret("c9", "budgets");
    let v: Value = serde_json::from_str(ret.trim()).unwrap();
    assert_eq!(v["t"], "ret");
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "port_unavailable");

    let hello = hello_params();
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    assert!(hello["host"]
        .as_str()
        .unwrap()
        .starts_with("medulla-public/"));
    assert!(hello["ports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "inference"));
}
