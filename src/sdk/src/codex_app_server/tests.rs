//! Unit tests for the pooled app-server client's pure parts: the line-framed
//! JSON-RPC dialect, the sharing key, and the thread-parameter mapping.
//!
//! The process-driving halves ([`super::connection`], [`super::pool`]) need a
//! child to talk to and are covered end-to-end by `tests/e2e_codex_app_server.rs`
//! against a scripted fake server.

use std::collections::HashMap;

use serde_json::json;

use super::jsonrpc::{notification_line, request_line, response_line, Message, RequestId};
use super::records::{read_record, Record, MAX_LINE_BYTES};
use super::types::{
    AppServerKey, AppServerSpec, ApprovalPolicy, SandboxMode, ThreadOptions, TurnStatus,
};

/// A spec with the given environment, over a fixed binary.
fn spec(env: &[(&str, &str)]) -> AppServerSpec {
    AppServerSpec {
        bin: "codex".to_string(),
        args: Vec::new(),
        env: env
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

#[test]
fn parses_a_response_by_id() {
    let message = Message::parse(r#"{"id":7,"result":{"ok":true}}"#).expect("a message");
    match message {
        Message::Response { id, result } => {
            assert_eq!(id, RequestId::Number(7));
            assert_eq!(result, json!({ "ok": true }));
        }
        other => panic!("expected a response, got {other:?}"),
    }
}

#[test]
fn parses_an_error_response_with_its_data() {
    let message =
        Message::parse(r#"{"id":"a","error":{"message":"nope","data":"why"}}"#).expect("a message");
    match message {
        Message::Error { id, message } => {
            assert_eq!(id, RequestId::Text("a".to_string()));
            assert_eq!(message, "nope (\"why\")");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn parses_a_notification_and_finds_its_thread() {
    let message =
        Message::parse(r#"{"method":"turn/started","params":{"threadId":"t1"}}"#).expect("parsed");
    match message {
        Message::Notification(notification) => {
            assert_eq!(notification.method, "turn/started");
            assert_eq!(notification.thread_id(), Some("t1"));
        }
        other => panic!("expected a notification, got {other:?}"),
    }
}

/// `thread/started` nests its id under `thread` rather than carrying `threadId`,
/// so the fan-out rule has to read both shapes.
#[test]
fn finds_the_thread_of_a_thread_started_notification() {
    let message =
        Message::parse(r#"{"method":"thread/started","params":{"thread":{"id":"t2"}}}"#).unwrap();
    let Message::Notification(notification) = message else {
        panic!("expected a notification");
    };
    assert_eq!(notification.thread_id(), Some("t2"));
}

#[test]
fn parses_a_server_request() {
    let line =
        r#"{"id":3,"method":"item/commandExecution/requestApproval","params":{"threadId":"t"}}"#;
    match Message::parse(line).expect("parsed") {
        Message::ServerRequest { id, method, params } => {
            assert_eq!(id, RequestId::Number(3));
            assert_eq!(method, "item/commandExecution/requestApproval");
            assert_eq!(params.get("threadId").unwrap(), "t");
        }
        other => panic!("expected a server request, got {other:?}"),
    }
}

/// Anything unrecognised is dropped rather than failing the stream: a running
/// turn must not die over a line nobody was waiting for.
#[test]
fn drops_lines_that_are_not_messages() {
    assert!(Message::parse("").is_none());
    assert!(Message::parse("   ").is_none());
    assert!(Message::parse("not json").is_none());
    assert!(Message::parse(r#"{"hello":"world"}"#).is_none());
}

#[test]
fn writes_the_jsonrpc_field_on_every_outbound_line() {
    for line in [
        request_line(&RequestId::Number(1), "initialize", json!({})),
        notification_line("initialized", json!({})),
        response_line(&RequestId::Number(1), json!({ "decision": "accept" })),
    ] {
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert_eq!(value.get("jsonrpc").unwrap(), "2.0");
        assert!(!line.contains('\n'), "lines carry no embedded newline");
    }
}

/// Two runs that differ only in variables nobody's identity depends on must land
/// on the same process, or pooling never happens in practice — almost every task
/// arrives with a different `PWD`.
#[test]
fn ignores_environment_that_does_not_change_who_the_process_is() {
    let a = AppServerKey::from_spec(&spec(&[("CODEX_HOME", "/home/a/.codex"), ("PWD", "/one")]));
    let b = AppServerKey::from_spec(&spec(&[("CODEX_HOME", "/home/a/.codex"), ("PWD", "/two")]));
    assert_eq!(a, b);
}

#[test]
fn separates_processes_that_would_authenticate_differently() {
    let a = AppServerKey::from_spec(&spec(&[("CODEX_HOME", "/home/a/.codex")]));
    let b = AppServerKey::from_spec(&spec(&[("CODEX_HOME", "/home/b/.codex")]));
    assert_ne!(a, b);

    let keyed = AppServerKey::from_spec(&spec(&[("OPENAI_API_KEY", "one")]));
    let other = AppServerKey::from_spec(&spec(&[("OPENAI_API_KEY", "two")]));
    assert_ne!(keyed, other);
}

#[test]
fn separates_processes_spawned_from_different_binaries() {
    let mut other = spec(&[]);
    other.bin = "/opt/codex-next/bin/codex".to_string();
    assert_ne!(
        AppServerKey::from_spec(&spec(&[])),
        AppServerKey::from_spec(&other)
    );
}

/// A key is written into logs, and the identity list it is built from holds API
/// keys.
#[test]
fn never_puts_a_credential_in_a_label() {
    let key = AppServerKey::from_spec(&spec(&[
        ("CODEX_HOME", "/home/a/.codex"),
        ("OPENAI_API_KEY", "sk-secret-value"),
    ]));
    let label = key.label();
    assert!(label.contains("/home/a/.codex"), "{label}");
    assert!(!label.contains("sk-secret-value"), "{label}");
}

#[test]
fn grants_a_consented_task_the_access_a_delegated_run_needs() {
    let options = ThreadOptions::from_permissions("/w".into(), None, true);
    assert_eq!(options.sandbox, SandboxMode::DangerFullAccess);
    assert_eq!(options.approval_policy, ApprovalPolicy::Never);
}

#[test]
fn confines_a_task_the_operator_did_not_consent_to() {
    let options = ThreadOptions::from_permissions("/w".into(), Some("gpt-5".into()), false);
    assert_eq!(options.sandbox, SandboxMode::WorkspaceWrite);
    assert_eq!(options.approval_policy, ApprovalPolicy::OnRequest);
    assert_eq!(options.model.as_deref(), Some("gpt-5"));
}

/// The sandbox and approval values are wire strings Codex parses, so a rename
/// that broke them would otherwise only show up against a live server.
#[test]
fn serializes_the_sandbox_and_approval_wire_spellings() {
    assert_eq!(json!(SandboxMode::ReadOnly), json!("read-only"));
    assert_eq!(json!(SandboxMode::WorkspaceWrite), json!("workspace-write"));
    assert_eq!(
        json!(SandboxMode::DangerFullAccess),
        json!("danger-full-access")
    );
    assert_eq!(json!(ApprovalPolicy::OnRequest), json!("on-request"));
    assert_eq!(json!(ApprovalPolicy::Never), json!("never"));
}

/// `inProgress` is not terminal and anything unknown must not read as success.
#[test]
fn treats_an_unknown_turn_status_as_a_failure() {
    assert_eq!(TurnStatus::from_wire("completed"), TurnStatus::Completed);
    assert_eq!(
        TurnStatus::from_wire("interrupted"),
        TurnStatus::Interrupted
    );
    assert_eq!(TurnStatus::from_wire("failed"), TurnStatus::Failed);
    assert_eq!(TurnStatus::from_wire("inProgress"), TurnStatus::Failed);
    assert_eq!(TurnStatus::from_wire("something-new"), TurnStatus::Failed);
}

/// A record within the cap frames normally, terminator stripped either way.
#[tokio::test]
async fn frames_complete_records_off_stdout() {
    let mut reader = std::io::Cursor::new(b"{\"a\":1}\n{\"b\":2}\r\n".to_vec());
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Line("{\"a\":1}".to_string())
    );
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Line("{\"b\":2}".to_string())
    );
    assert_eq!(read_record(&mut reader).await.expect("eof"), Record::Eof);
}

/// A final record with no trailing newline is still worth acting on; EOF is
/// reported on the call after it, not instead of it.
#[tokio::test]
async fn yields_an_unterminated_final_record_before_eof() {
    let mut reader = std::io::Cursor::new(b"{\"a\":1}".to_vec());
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Line("{\"a\":1}".to_string())
    );
    assert_eq!(read_record(&mut reader).await.expect("eof"), Record::Eof);
}

/// The point of the whole module: an oversized record is discarded as it
/// arrives rather than buffered, and framing resynchronises on its newline so
/// the connection survives it.
#[tokio::test]
async fn discards_an_oversized_record_and_resynchronises() {
    let mut bytes = vec![b'x'; MAX_LINE_BYTES + 1];
    bytes.push(b'\n');
    bytes.extend_from_slice(b"{\"after\":true}\n");
    let mut reader = std::io::Cursor::new(bytes);
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Oversized
    );
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Line("{\"after\":true}".to_string())
    );
}

/// An oversized record that never terminates ends the stream as oversized
/// rather than as a giant line.
#[tokio::test]
async fn reports_an_unterminated_oversized_record_at_eof() {
    let mut reader = std::io::Cursor::new(vec![b'x'; MAX_LINE_BYTES + 1]);
    assert_eq!(
        read_record(&mut reader).await.expect("a record"),
        Record::Oversized
    );
    assert_eq!(read_record(&mut reader).await.expect("eof"), Record::Eof);
}

/// Pinned argv is part of the sharing key: two runs configured differently must
/// not land on one process, or one inherits the other's routing.
#[test]
fn separates_connections_that_pinned_different_argv() {
    let mut routed = spec(&[]);
    routed.args = vec!["-c".to_string(), "model_provider=\"medulla\"".to_string()];
    assert_ne!(
        AppServerKey::from_spec(&routed),
        AppServerKey::from_spec(&spec(&[]))
    );
}
