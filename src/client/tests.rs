//! Unit and integration tests for the Medulla client.

use super::*;
use crate::client::sse::{SeqDedup, SseFrame, SseParser};
use futures::StreamExt;
use serde_json::json;

// ---------------------------------------------------------------------------
// SSE parser
// ---------------------------------------------------------------------------

fn parse_all(input: &str) -> Vec<SseFrame> {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    parser.feed(input, &mut out);
    out
}

#[test]
fn parses_id_and_data_frame() {
    let frames = parse_all("id: 42\ndata: {\"a\":1}\n\n");
    assert_eq!(
        frames,
        vec![SseFrame {
            id: Some(42),
            data: "{\"a\":1}".to_string(),
        }]
    );
}

#[test]
fn ignores_ping_comments() {
    let frames = parse_all(": ping\n\ndata: hi\n\n");
    assert_eq!(
        frames,
        vec![SseFrame {
            id: None,
            data: "hi".to_string(),
        }]
    );
}

#[test]
fn concatenates_multiline_data() {
    let frames = parse_all("data: line1\ndata: line2\n\n");
    assert_eq!(
        frames,
        vec![SseFrame {
            id: None,
            data: "line1\nline2".to_string(),
        }]
    );
}

#[test]
fn handles_chunked_and_crlf_boundaries() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    // Split a single frame across several feeds, with CRLF line endings.
    parser.feed("id: 7\r\nda", &mut out);
    parser.feed("ta: {\"x\":", &mut out);
    parser.feed("2}\r\n\r\n", &mut out);
    assert_eq!(
        out,
        vec![SseFrame {
            id: Some(7),
            data: "{\"x\":2}".to_string(),
        }]
    );
}

#[test]
fn yields_multiple_frames() {
    let frames = parse_all("id: 1\ndata: a\n\nid: 2\ndata: b\n\n");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].id, Some(1));
    assert_eq!(frames[1].id, Some(2));
}

// ---------------------------------------------------------------------------
// Reconnect dedupe cursor
// ---------------------------------------------------------------------------

#[test]
fn dedupe_skips_replayed_seqs_and_advances_cursor() {
    let mut d = SeqDedup::new(Some(2));
    assert!(!d.accept(Some(1))); // replayed, below cursor
    assert!(!d.accept(Some(2))); // equal to cursor
    assert!(d.accept(Some(3))); // new
    assert_eq!(d.cursor(), Some(3));
    assert!(d.accept(None)); // deltas always pass
    assert!(!d.accept(Some(3))); // duplicate
    assert!(d.accept(Some(4)));
    assert_eq!(d.cursor(), Some(4));
}

#[test]
fn dedupe_from_start_accepts_everything() {
    let mut d = SeqDedup::new(None);
    assert!(d.accept(Some(0)));
    assert!(d.accept(Some(1)));
    assert!(!d.accept(Some(1)));
}

// ---------------------------------------------------------------------------
// Event envelope / kind decode fixtures
// ---------------------------------------------------------------------------

fn envelope(event: Value) -> EventEnvelope {
    let raw = json!({
        "seq": 5,
        "at": 1234,
        "sessionId": "s1",
        "cycleId": "c1",
        "event": event,
    });
    serde_json::from_value(raw).unwrap()
}

#[test]
fn decodes_user_and_assistant() {
    assert_eq!(
        envelope(json!({"kind": "user", "body": "hi"})).kind(),
        EventKind::User { body: "hi".into() }
    );
    assert_eq!(
        envelope(json!({"kind": "assistant", "body": "yo"})).kind(),
        EventKind::Assistant { body: "yo".into() }
    );
}

#[test]
fn decodes_cycle_bracket() {
    assert_eq!(
        envelope(json!({"kind": "cycle_start", "cycleId": "c1"})).kind(),
        EventKind::CycleStart {
            cycle_id: Some("c1".into())
        }
    );
    assert_eq!(
        envelope(json!({"kind": "cycle_end", "cycleId": "c1", "passCount": 3, "durationMs": 120}))
            .kind(),
        EventKind::CycleEnd {
            cycle_id: Some("c1".into()),
            pass_count: Some(3),
            duration_ms: Some(120),
            error: None,
        }
    );
    assert_eq!(
        envelope(json!({"kind": "cycle_end", "cycleId": "c1", "error": true})).kind(),
        EventKind::CycleEnd {
            cycle_id: Some("c1".into()),
            pass_count: None,
            duration_ms: None,
            error: Some(true),
        }
    );
}

#[test]
fn decodes_error_and_deltas() {
    assert_eq!(
        envelope(json!({"kind": "error", "source": "cycle", "message": "boom"})).kind(),
        EventKind::Error {
            source: "cycle".into(),
            message: "boom".into(),
        }
    );
    assert_eq!(
        envelope(json!({"kind": "assistant_delta", "delta": "to"})).kind(),
        EventKind::AssistantDelta { delta: "to".into() }
    );
    assert_eq!(
        envelope(json!({"kind": "reasoning_delta", "delta": "hm"})).kind(),
        EventKind::ReasoningDelta { delta: "hm".into() }
    );
    match envelope(json!({"kind": "tool_call_delta", "id": "t1"})).kind() {
        EventKind::ToolCallDelta { value } => assert_eq!(value["id"], json!("t1")),
        other => panic!("expected tool_call_delta, got {other:?}"),
    }
}

#[test]
fn unknown_kind_passthrough_preserves_raw() {
    let ev = envelope(json!({"kind": "future_thing", "payload": 9}));
    match ev.kind() {
        EventKind::Unknown(v) => {
            assert_eq!(v["kind"], json!("future_thing"));
            assert_eq!(v["payload"], json!(9));
        }
        other => panic!("expected unknown, got {other:?}"),
    }
    // Raw value stays accessible on the envelope.
    assert_eq!(ev.event["payload"], json!(9));
    assert_eq!(ev.seq, Some(5));
}

// ---------------------------------------------------------------------------
// Envelope unwrapping / error mapping
// ---------------------------------------------------------------------------

#[test]
fn unwraps_success_envelope() {
    let body = br#"{"success":true,"data":{"sessionId":"abc"}}"#;
    let out: SessionCreated = unwrap_envelope(201, body).unwrap();
    assert_eq!(out.session_id, "abc");
}

#[test]
fn maps_error_envelope_with_code() {
    let body = br#"{"success":false,"error":"token expired","errorCode":"TOKEN_EXPIRED"}"#;
    let err = unwrap_envelope::<Value>(401, body).unwrap_err();
    assert_eq!(err.error_code(), Some("TOKEN_EXPIRED"));
    assert!(err.is_token_expired());
    assert_eq!(err.status(), Some(401));
    match err {
        ClientError::Api { message, .. } => assert_eq!(message, "token expired"),
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn maps_error_envelope_with_details() {
    let body = br#"{"success":false,"error":"bad","errorCode":"PROTOCOL_MISMATCH","details":{"min":1,"max":2}}"#;
    let err = unwrap_envelope::<Value>(409, body).unwrap_err();
    match err {
        ClientError::Api { details, .. } => {
            let d = details.unwrap();
            assert_eq!(d["min"], json!(1));
            assert_eq!(d["max"], json!(2));
        }
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn non_json_error_body_becomes_api_error() {
    let err = unwrap_envelope::<Value>(500, b"internal error").unwrap_err();
    assert_eq!(err.status(), Some(500));
    assert_eq!(err.error_code(), None);
}

// ---------------------------------------------------------------------------
// LoopEvent / run result decode
// ---------------------------------------------------------------------------

#[test]
fn decodes_loop_events() {
    let tool_use: LoopEvent = serde_json::from_value(json!({
        "stop": "tool_use",
        "cycleId": "c1",
        "sessionId": "s1",
        "toolCalls": [{"id": "t1", "name": "search", "args": {"q": "x"}}],
    }))
    .unwrap();
    match tool_use {
        LoopEvent::ToolUse { tool_calls, .. } => {
            assert_eq!(tool_calls[0].name, "search");
            assert_eq!(tool_calls[0].args["q"], json!("x"));
        }
        other => panic!("expected tool_use, got {other:?}"),
    }

    let end: LoopEvent = serde_json::from_value(json!({
        "stop": "end",
        "cycleId": "c1",
        "sessionId": "s1",
        "reply": "done",
        "passCount": 2,
    }))
    .unwrap();
    match end {
        LoopEvent::End {
            reply, pass_count, ..
        } => {
            assert_eq!(reply, "done");
            assert_eq!(pass_count, Some(2));
        }
        other => panic!("expected end, got {other:?}"),
    }

    let pending: LoopEvent =
        serde_json::from_value(json!({"stop": "pending", "cycleId": "c1", "sessionId": "s1"}))
            .unwrap();
    assert!(matches!(pending, LoopEvent::Pending { .. }));
}

#[test]
fn run_result_distinguishes_reply_and_loop() {
    let reply = parse_run_result(json!({
        "reply": "hello",
        "passCount": 1,
        "sessionId": "s1",
        "cycleId": "c1",
    }))
    .unwrap();
    assert!(matches!(reply, RunResult::Reply(_)));

    let looped = parse_run_result(json!({
        "stop": "end",
        "cycleId": "c1",
        "sessionId": "s1",
        "reply": "done",
    }))
    .unwrap();
    assert!(matches!(looped, RunResult::Loop(LoopEvent::End { .. })));
}

#[test]
fn run_options_serialize_camel_case() {
    let opts = RunOptions {
        session_id: Some("s1".into()),
        tools: None,
        options: Some(RunOrchestrationOptions {
            prompt_overrides: None,
            config: Some(RunConfig {
                max_passes: Some(4),
                ..Default::default()
            }),
            limits: None,
        }),
    };
    let v = serde_json::to_value(&opts).unwrap();
    assert_eq!(v["sessionId"], json!("s1"));
    assert_eq!(v["options"]["config"]["maxPasses"], json!(4));
    // Unset fields are omitted.
    assert!(v.get("tools").is_none());
}

// ---------------------------------------------------------------------------
// Integration: local TCP stub server
// ---------------------------------------------------------------------------

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Bind a stub server that handles one connection: drain the request, then
/// write `response` verbatim and close. Returns the bound address.
async fn spawn_stub(response: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // Read whatever the client sent (headers, possibly body); ignore it.
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        sock.write_all(&response).await.unwrap();
        sock.flush().await.unwrap();
        let _ = sock.shutdown().await;
    });
    format!("http://{addr}")
}

fn http_json(status_line: &str, body: &str) -> Vec<u8> {
    format!(
        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn integration_create_session_round_trip() {
    let body = r#"{"success":true,"data":{"sessionId":"sess-123"}}"#;
    let base = spawn_stub(http_json("HTTP/1.1 201 Created", body)).await;
    let client = MedullaClient::new(base, "jwt-abc");
    let created = client.create_session(Some("hello")).await.unwrap();
    assert_eq!(created.session_id, "sess-123");
}

#[tokio::test]
async fn integration_error_envelope_maps() {
    let body = r#"{"success":false,"error":"nope","errorCode":"TOKEN_EXPIRED"}"#;
    let base = spawn_stub(http_json("HTTP/1.1 401 Unauthorized", body)).await;
    let client = MedullaClient::new(base, "jwt-abc");
    let err = client.me().await.unwrap_err();
    assert!(err.is_token_expired());
}

#[tokio::test]
async fn integration_sse_stream_yields_frames() {
    // SSE response: headers, then two persisted event frames, then close.
    let frame = |seq: u64, body: &str| {
        format!(
            "id: {seq}\ndata: {{\"seq\":{seq},\"at\":1,\"sessionId\":\"s1\",\"event\":{{\"kind\":\"assistant\",\"body\":\"{body}\"}}}}\n\n"
        )
    };
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(": ping\n\n".as_bytes());
    response.extend_from_slice(frame(1, "one").as_bytes());
    response.extend_from_slice(frame(2, "two").as_bytes());
    let base = spawn_stub(response).await;

    let client = MedullaClient::new(base, "jwt-abc");
    let stream = client.stream_events("s1", None);
    futures::pin_mut!(stream);

    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.seq, Some(1));
    assert_eq!(first.kind(), EventKind::Assistant { body: "one".into() });

    let second = stream.next().await.unwrap().unwrap();
    assert_eq!(second.seq, Some(2));
    assert_eq!(second.kind(), EventKind::Assistant { body: "two".into() });
    // Stop by dropping the stream.
}

// ---------------------------------------------------------------------------
// Integration: full endpoint surface against the TCP stub
// ---------------------------------------------------------------------------

/// Like [`spawn_stub`], but also hands back the raw request bytes the client
/// sent so tests can assert on the method line, query string, and body.
async fn spawn_stub_capture(response: Vec<u8>) -> (String, tokio::sync::oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        sock.write_all(&response).await.unwrap();
        sock.flush().await.unwrap();
        let _ = sock.shutdown().await;
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
    });
    (format!("http://{addr}"), rx)
}

/// A loopback address with nothing listening (bound then immediately released),
/// so a connect attempt is refused.
async fn dead_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

fn ok_envelope(status: &str, data: serde_json::Value) -> Vec<u8> {
    http_json(
        status,
        &json!({ "success": true, "data": data }).to_string(),
    )
}

#[tokio::test]
async fn consume_login_token_returns_jwt() {
    let (base, req) =
        spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", json!({ "jwt": "jwt-xyz" }))).await;
    let client = MedullaClient::new(base, "");
    let jwt = client.consume_login_token("one-time").await.unwrap();
    assert_eq!(jwt, "jwt-xyz");
    let sent = req.await.unwrap();
    assert!(sent.starts_with("POST /auth/login-token/consume"), "{sent}");
    assert!(sent.contains("\"token\":\"one-time\""), "{sent}");
}

#[tokio::test]
async fn me_carries_bearer_and_unwraps() {
    let (base, req) =
        spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", json!({ "sub": "user-1" }))).await;
    let client = MedullaClient::new(base, "jwt-abc");
    let me = client.me().await.unwrap();
    assert_eq!(me["sub"], json!("user-1"));
    let sent = req.await.unwrap();
    assert!(sent.contains("authorization: Bearer jwt-abc"), "{sent}");
}

#[tokio::test]
async fn list_sessions_decodes_rows() {
    let data = json!([
        { "sessionId": "s1", "title": "One", "status": "active", "lastSeq": 4 },
        { "sessionId": "s2", "status": "idle" },
    ]);
    let (base, _req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let rows = client.list_sessions().await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].session_id, "s1");
    assert_eq!(rows[0].title.as_deref(), Some("One"));
    assert_eq!(rows[1].status, SessionStatus::Idle);
}

#[tokio::test]
async fn get_session_decodes_detail() {
    let data = json!({ "sessionId": "s9", "status": "active", "eventSeq": 12, "lastSeq": 12 });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let detail = client.get_session("s9").await.unwrap();
    assert_eq!(detail.session_id, "s9");
    assert_eq!(detail.event_seq, Some(12));
    let sent = req.await.unwrap();
    assert!(sent.starts_with("GET /medulla/v1/sessions/s9 "), "{sent}");
}

#[tokio::test]
async fn archive_session_round_trip() {
    let data = json!({ "sessionId": "s3", "status": "archived" });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let out = client.archive_session("s3").await.unwrap();
    assert_eq!(out.session_id, "s3");
    assert_eq!(out.status, SessionStatus::Archived);
    let sent = req.await.unwrap();
    assert!(sent.starts_with("DELETE /medulla/v1/sessions/s3"), "{sent}");
}

#[tokio::test]
async fn send_message_async_sets_sync_zero() {
    let data = json!({ "cycleId": "c1", "seq": 7 });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 202 Accepted", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let out = client.send_message("s1", "hello", false).await.unwrap();
    assert_eq!(out.cycle_id, "c1");
    assert_eq!(out.seq, 7);
    assert!(out.reply.is_none());
    let sent = req.await.unwrap();
    assert!(sent.contains("sync=0"), "{sent}");
    assert!(sent.contains("\"body\":\"hello\""), "{sent}");
}

#[tokio::test]
async fn send_message_sync_carries_reply_and_flag() {
    let data = json!({ "cycleId": "c2", "seq": 9, "reply": "done" });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let out = client.send_message("s1", "hi", true).await.unwrap();
    assert_eq!(out.reply.as_deref(), Some("done"));
    let sent = req.await.unwrap();
    assert!(sent.contains("sync=1"), "{sent}");
}

#[tokio::test]
async fn list_messages_passes_after_cursor() {
    let data = json!([{ "seq": 3, "role": "user", "body": "q", "ts": 100 }]);
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let msgs = client.list_messages("s1", Some(2)).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[0].seq, 3);
    let sent = req.await.unwrap();
    assert!(sent.contains("after=2"), "{sent}");
}

#[tokio::test]
async fn list_events_decodes_envelopes() {
    let data = json!([
        { "seq": 1, "at": 10, "sessionId": "s1", "event": { "kind": "assistant", "body": "hi" } },
    ]);
    let (base, _req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let evs = client.list_events("s1", None).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].kind(), EventKind::Assistant { body: "hi".into() });
}

#[tokio::test]
async fn abort_round_trip() {
    let data = json!({ "sessionId": "s1", "aborted": true });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let out = client.abort("s1").await.unwrap();
    assert!(out.aborted);
    let sent = req.await.unwrap();
    assert!(
        sent.starts_with("POST /medulla/v1/sessions/s1/abort"),
        "{sent}"
    );
}

#[tokio::test]
async fn run_returns_reply_when_toolless() {
    let data = json!({ "reply": "hello", "passCount": 1, "sessionId": "s1", "cycleId": "c1" });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let out = client.run("go", RunOptions::default()).await.unwrap();
    match out {
        RunResult::Reply(r) => {
            assert_eq!(r.reply, "hello");
            assert_eq!(r.pass_count, Some(1));
        }
        other => panic!("expected reply, got {other:?}"),
    }
    let sent = req.await.unwrap();
    assert!(sent.starts_with("POST /orchestration/v1/run"), "{sent}");
    assert!(sent.contains("\"input\":\"go\""), "{sent}");
}

#[tokio::test]
async fn run_returns_loop_when_tools_present() {
    let data = json!({
        "stop": "tool_use",
        "cycleId": "c1",
        "sessionId": "s1",
        "toolCalls": [{ "id": "t1", "name": "search", "args": { "q": "x" } }],
    });
    let (base, _req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let opts = RunOptions {
        tools: Some(vec![ToolDef {
            name: "search".into(),
            description: "web".into(),
            parameters: json!({ "type": "object" }),
        }]),
        ..Default::default()
    };
    let out = client.run("go", opts).await.unwrap();
    match out {
        RunResult::Loop(LoopEvent::ToolUse { tool_calls, .. }) => {
            assert_eq!(tool_calls[0].name, "search");
        }
        other => panic!("expected tool_use loop, got {other:?}"),
    }
}

#[tokio::test]
async fn continue_run_decodes_loop_event() {
    let data = json!({ "stop": "end", "cycleId": "c1", "sessionId": "s1", "reply": "fin" });
    let (base, req) = spawn_stub_capture(ok_envelope("HTTP/1.1 200 OK", data)).await;
    let client = MedullaClient::new(base, "jwt");
    let results = vec![ToolResult {
        id: "t1".into(),
        ok: true,
        result: Some(json!({ "answer": 42 })),
        error: None,
    }];
    let ev = client.continue_run("c1", results).await.unwrap();
    match ev {
        LoopEvent::End { reply, .. } => assert_eq!(reply, "fin"),
        other => panic!("expected end, got {other:?}"),
    }
    let sent = req.await.unwrap();
    assert!(sent.contains("\"cycleId\":\"c1\""), "{sent}");
    assert!(sent.contains("\"toolResults\""), "{sent}");
}

#[tokio::test]
async fn http_error_without_envelope_becomes_api_error() {
    let (base, _req) = spawn_stub_capture(http_json(
        "HTTP/1.1 503 Service Unavailable",
        "upstream down",
    ))
    .await;
    let client = MedullaClient::new(base, "jwt");
    let err = client.get_session("s1").await.unwrap_err();
    assert_eq!(err.status(), Some(503));
    assert_eq!(err.error_code(), None);
    match err {
        ClientError::Api { message, .. } => assert_eq!(message, "upstream down"),
        other => panic!("expected api error, got {other:?}"),
    }
}

#[tokio::test]
async fn success_status_with_non_json_body_is_decode_error() {
    let (base, _req) = spawn_stub_capture(http_json("HTTP/1.1 200 OK", "not json at all")).await;
    let client = MedullaClient::new(base, "jwt");
    let err = client.me().await.unwrap_err();
    assert!(matches!(err, ClientError::Decode(_)), "got {err:?}");
}

#[tokio::test]
async fn connection_refused_surfaces_transport_error() {
    let base = dead_addr().await;
    let client = MedullaClient::new(base, "jwt");
    let err = client.list_sessions().await.unwrap_err();
    assert!(matches!(err, ClientError::Transport(_)), "got {err:?}");
    assert_eq!(err.status(), None);
    assert_eq!(err.error_code(), None);
}

// ---------------------------------------------------------------------------
// SSE stream edge cases (decode error, empty-data skip, connect failure)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_surfaces_a_decode_error_for_bad_json() {
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(b": ping\n\n");
    response.extend_from_slice(b"data: {not valid json}\n\n");
    let base = spawn_stub(response).await;
    let client = MedullaClient::new(base, "jwt");
    let stream = client.stream_events("s1", None);
    futures::pin_mut!(stream);
    let first = stream.next().await.unwrap();
    assert!(matches!(first, Err(ClientError::Decode(_))), "{first:?}");
}

#[tokio::test]
async fn sse_skips_empty_data_frames() {
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_vec();
    // An id-only frame and an empty-data frame both carry no payload → skipped.
    response.extend_from_slice(b"data: \n\n");
    response.extend_from_slice(
        b"id: 5\ndata: {\"seq\":5,\"at\":1,\"sessionId\":\"s1\",\"event\":{\"kind\":\"assistant\",\"body\":\"real\"}}\n\n",
    );
    let base = spawn_stub(response).await;
    let client = MedullaClient::new(base, "jwt");
    let stream = client.stream_events("s1", None);
    futures::pin_mut!(stream);
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.seq, Some(5));
    assert_eq!(
        first.kind(),
        EventKind::Assistant {
            body: "real".into()
        }
    );
}

#[tokio::test]
async fn sse_connect_failure_surfaces_transport_error() {
    // A non-2xx status on the stream GET fails `error_for_status`.
    let base = spawn_stub(http_json("HTTP/1.1 500 Internal Server Error", "boom")).await;
    let client = MedullaClient::new(base, "jwt");
    let stream = client.stream_events("s1", None);
    futures::pin_mut!(stream);
    let first = stream.next().await.unwrap();
    assert!(matches!(first, Err(ClientError::Transport(_))), "{first:?}");
}
