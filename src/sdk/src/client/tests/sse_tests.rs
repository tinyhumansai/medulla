//! SSE-focused tests: the incremental frame parser, the reconnect dedupe
//! cursor, and end-to-end streaming (including decode/empty/connect edges)
//! driven through the shared TCP stub.

use super::{http_json, spawn_stub};
use crate::client::sse::{SeqDedup, SseFrame, SseParser, MAX_FRAME_BYTES};
use crate::client::*;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// SSE parser
// ---------------------------------------------------------------------------

fn parse_all(input: &str) -> Vec<SseFrame> {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    parser.feed(input, &mut out).expect("no overflow");
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
    parser.feed("id: 7\r\nda", &mut out).unwrap();
    parser.feed("ta: {\"x\":", &mut out).unwrap();
    parser.feed("2}\r\n\r\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: Some(7),
            data: "{\"x\":2}".to_string(),
        }]
    );
}

#[test]
fn reset_drops_a_truncated_frames_state() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    // Connection dies mid-frame.
    parser.feed("id: 3\ndata: {\"par", &mut out).unwrap();
    assert!(out.is_empty());
    parser.reset();
    // The reconnect replays the event in full; nothing may be spliced on.
    parser.feed("id: 3\ndata: whole\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: Some(3),
            data: "whole".to_string(),
        }]
    );
}

#[test]
fn multi_byte_characters_survive_a_chunk_boundary() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let payload = "data: héllo→\n\n".as_bytes();
    // Feed one byte at a time — the worst case a transport can produce.
    for byte in payload {
        parser.feed_bytes(&[*byte], &mut out).unwrap();
    }
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "héllo→".to_string(),
        }]
    );
}

#[test]
fn invalid_utf8_becomes_one_replacement_char() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let mut bytes = b"data: a".to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(b"b\n\n");
    parser.feed_bytes(&bytes, &mut out).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].data, "a\u{fffd}b");
}

#[test]
fn a_line_without_a_newline_is_capped() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let flood = "x".repeat(MAX_FRAME_BYTES + 1);
    assert!(parser.feed(&flood, &mut out).is_err());
    assert!(out.is_empty());
    // Recovery resumes at the next frame boundary, discarding the flood's tail.
    parser.feed("more junk\n\ndata: ok\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "ok".to_string(),
        }]
    );
}

#[test]
fn an_unterminated_oversized_line_discards_to_its_blank_line() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    // The oversized line is cut mid-flight, before its terminating newline, so
    // the parser is still inside the discarded line when the cap fires.
    let flood = "data: ".to_string() + &"x".repeat(MAX_FRAME_BYTES + 1);
    assert!(parser.feed(&flood, &mut out).is_err());
    assert!(out.is_empty());
    // The next chunk begins with that line's own newline, then carries a data
    // line that still belongs to the discarded oversized frame (no blank line
    // in between). It must not be emitted as a valid event — only the frame's
    // terminating blank line may end the discard.
    parser.feed("\ndata: sneaky\n\n", &mut out).unwrap();
    assert!(out.is_empty());
    // A genuinely new frame after the discarded one still parses.
    parser.feed("data: ok\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "ok".to_string(),
        }]
    );
}

#[test]
fn an_oversized_tail_with_its_own_newline_still_ends_the_discarded_line() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    // An oversized line cut mid-flight sets in_discarded_line.
    let flood = format!("data: {}", "x".repeat(MAX_FRAME_BYTES + 1));
    assert!(parser.feed(&flood, &mut out).is_err());
    // Its tail is itself larger than the cap before ending in a newline, so the
    // newline-terminated oversized branch consumes the terminator. That newline
    // finishes the discarded line, so the first blank line to arrive afterwards
    // is the frame boundary again — not another line terminator.
    let tail = format!("{}\n", "y".repeat(MAX_FRAME_BYTES + 1));
    assert!(parser.feed(&tail, &mut out).is_err());
    // The frame's blank boundary ends the discard; a genuinely new frame parses.
    parser.feed("\ndata: ok\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "ok".to_string(),
        }]
    );
}

#[test]
fn an_oversized_newline_terminated_line_is_capped_too() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    // A single chunk can carry a newline-terminated line longer than the cap,
    // which the no-newline branch cannot see. A comment line is never
    // length-checked downstream, so without an explicit cap it would be cloned
    // into `line` in full and accepted — an allocation spike from one chunk.
    let flood = format!(": {}\n", "x".repeat(MAX_FRAME_BYTES + 1));
    assert!(parser.feed(&flood, &mut out).is_err());
    assert!(out.is_empty());
    // Recovery resumes at the next frame boundary.
    parser.feed("\ndata: ok\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "ok".to_string(),
        }]
    );
}

#[test]
fn an_oversized_payload_is_dropped_not_accumulated() {
    let mut parser = SseParser::new();
    let mut out = Vec::new();
    let line = format!("data: {}\n", "y".repeat(MAX_FRAME_BYTES / 2));
    // The first line fits; the second pushes the payload past the cap.
    parser.feed(&line, &mut out).unwrap();
    assert!(parser.feed(&line, &mut out).is_err());
    parser.feed("\ndata: ok\n\n", &mut out).unwrap();
    assert_eq!(
        out,
        vec![SseFrame {
            id: None,
            data: "ok".to_string(),
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
// Integration: SSE streaming through the TCP stub
// ---------------------------------------------------------------------------

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

#[tokio::test]
async fn sse_requests_identify_medulla() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let read = socket.read(&mut buf).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"at\":1,\"sessionId\":\"s1\",\"event\":{\"kind\":\"assistant\",\"body\":\"ok\"}}\n\n",
            )
            .await
            .unwrap();
        socket.flush().await.unwrap();
        let _ = tx.send(String::from_utf8_lossy(&buf[..read]).to_string());
    });

    let client = MedullaClient::new(format!("http://{addr}"), "jwt-abc");
    let stream = client.stream_events("s1", None);
    futures::pin_mut!(stream);
    stream.next().await.unwrap().unwrap();

    let sent = rx.await.unwrap();
    assert!(sent.contains("x-sdk-name: medulla"), "{sent}");
    assert!(!sent.contains("x-sdk-name: openhuman"), "{sent}");
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
