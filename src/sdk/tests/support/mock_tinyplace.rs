//! A minimal in-test mock of the tiny.place backend HTTP API.
//!
//! Speaks just enough HTTP/1.1 (one request per connection, `Connection: close`)
//! to satisfy the endpoints the vendored `tinyplace` SDK client calls from the
//! medulla runtime helpers and [`TinyplaceService`](medulla::tinyplace_support::service):
//!
//! - `POST /presence/heartbeat`   → `PresenceStatus` (or a scripted 5xx)
//! - `POST /presence/query`       → `PresenceQueryResponse`
//! - `GET  /contacts/requests`    → `ContactRequestsResponse`
//! - `POST /contacts/:id/accept`  → `Contact` (records the accepted id)
//! - `GET  /messages`             → `{ messages: [...] }` (the pending queue)
//! - `DELETE /messages/:id`       → 200 empty (drains that id from the queue)
//!
//! The SDK parses response bodies as the raw typed JSON (no `{success,data}`
//! envelope), so the mock returns bare objects. Auth headers are ignored — the
//! mock never verifies signatures. Every request is recorded for assertions.
//!
//! Hand-rolled in the same style as `mock_backend.rs`; kept separate so the
//! tinyplace e2e suite owns it.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One recorded inbound HTTP request.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    /// The path with query string stripped.
    pub path: String,
    /// The raw query string (without the leading `?`), empty when absent.
    pub query: String,
    pub body: String,
}

/// A pending inbound contact request the mock advertises on `GET /contacts/requests`.
#[derive(Clone)]
pub struct PendingRequest {
    pub agent_id: String,
    pub status: String,
}

impl PendingRequest {
    pub fn incoming(agent_id: &str) -> Self {
        PendingRequest {
            agent_id: agent_id.to_string(),
            status: "pending".to_string(),
        }
    }
}

/// A message the mock delivers on `GET /messages` until acknowledged.
#[derive(Clone)]
pub struct MockMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub body: String,
}

/// Scriptable mock behaviour.
#[derive(Clone, Default)]
pub struct MockConfig {
    /// Pending incoming contact requests returned by `GET /contacts/requests`.
    pub pending: Vec<PendingRequest>,
    /// Messages returned by `GET /messages` until drained by acknowledge.
    pub messages: Vec<MockMessage>,
    /// cryptoIds reported `online:true` by `POST /presence/query` (all requested
    /// ids get an entry; ones not listed here come back offline).
    pub online: Vec<String>,
    /// HTTP status the heartbeat endpoint returns (200 by default).
    pub heartbeat_status: u16,
}

struct MockState {
    requests: Mutex<Vec<RecordedRequest>>,
    config: Mutex<MockConfig>,
    accepted: Mutex<Vec<String>>,
    acknowledged: Mutex<Vec<String>>,
    heartbeats: Mutex<u32>,
    presence_queries: Mutex<u32>,
}

/// A running mock tiny.place backend. Drop it to stop the acceptor.
pub struct MockTinyplace {
    pub base_url: String,
    state: Arc<MockState>,
    _accept: JoinHandle<()>,
}

impl Drop for MockTinyplace {
    fn drop(&mut self) {
        self._accept.abort();
    }
}

impl MockTinyplace {
    /// Bind on an ephemeral loopback port and start accepting.
    pub async fn start(config: MockConfig) -> Self {
        let cfg = MockConfig {
            heartbeat_status: if config.heartbeat_status == 0 {
                200
            } else {
                config.heartbeat_status
            },
            ..config
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(MockState {
            requests: Mutex::new(Vec::new()),
            config: Mutex::new(cfg),
            accepted: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            heartbeats: Mutex::new(0),
            presence_queries: Mutex::new(0),
        });
        let accept_state = state.clone();
        let accept = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((sock, _)) => {
                        let st = accept_state.clone();
                        tokio::spawn(async move {
                            let _ = handle_conn(sock, st).await;
                        });
                    }
                    Err(_) => return,
                }
            }
        });
        MockTinyplace {
            base_url: format!("http://{addr}"),
            state,
            _accept: accept,
        }
    }

    /// Mutate the config live (e.g. flip `heartbeat_status`).
    pub fn configure(&self, f: impl FnOnce(&mut MockConfig)) {
        f(&mut self.state.config.lock().unwrap());
    }

    /// Every request seen so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.requests.lock().unwrap().clone()
    }

    /// cryptoIds accepted via `POST /contacts/:id/accept`.
    pub fn accepted(&self) -> Vec<String> {
        self.state.accepted.lock().unwrap().clone()
    }

    /// message ids acknowledged via `DELETE /messages/:id`.
    pub fn acknowledged(&self) -> Vec<String> {
        self.state.acknowledged.lock().unwrap().clone()
    }

    /// How many heartbeats have been received.
    pub fn heartbeats(&self) -> u32 {
        *self.state.heartbeats.lock().unwrap()
    }

    /// How many presence queries have been received.
    pub fn presence_queries(&self) -> u32 {
        *self.state.presence_queries.lock().unwrap()
    }
}

/// Poll `predicate` until it returns true or `timeout` elapses. Panics on
/// timeout. Short interval to keep wall time small.
pub async fn wait_until<F>(label: &str, timeout: Duration, mut predicate: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {label}");
}

async fn handle_conn(mut sock: TcpStream, state: Arc<MockState>) -> std::io::Result<()> {
    let Some((method, raw_path, body)) = read_request(&mut sock).await? else {
        return Ok(());
    };
    let (path, query) = match raw_path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_path.clone(), String::new()),
    };
    state.requests.lock().unwrap().push(RecordedRequest {
        method: method.clone(),
        path: path.clone(),
        query: query.clone(),
        body: body.clone(),
    });

    let (status, response_body) = route(&method, &path, &query, &state);
    respond(&mut sock, status, &response_body).await
}

fn route(method: &str, path: &str, query: &str, state: &Arc<MockState>) -> (&'static str, String) {
    // Presence heartbeat.
    if method == "POST" && path == "/presence/heartbeat" {
        *state.heartbeats.lock().unwrap() += 1;
        let code = state.config.lock().unwrap().heartbeat_status;
        if code >= 400 {
            return (
                status_text(code),
                json!({ "error": "heartbeat down" }).to_string(),
            );
        }
        let snapshot = json!({ "cryptoId": "@self", "online": true });
        return ("200 OK", snapshot.to_string());
    }

    // Presence batch query.
    if method == "POST" && path == "/presence/query" {
        *state.presence_queries.lock().unwrap() += 1;
        let online = state.config.lock().unwrap().online.clone();
        let requested = requested_crypto_ids(&state_last_body(state));
        let presence: Vec<Value> = requested
            .iter()
            .map(|id| json!({ "cryptoId": id, "online": online.contains(id) }))
            .collect();
        return ("200 OK", json!({ "presence": presence }).to_string());
    }

    // Contact requests listing.
    if method == "GET" && path == "/contacts/requests" {
        let pending = state.config.lock().unwrap().pending.clone();
        let incoming: Vec<Value> = pending
            .iter()
            .map(|p| json!({ "cryptoId": p.agent_id, "status": p.status, "direction": "incoming" }))
            .collect();
        return (
            "200 OK",
            json!({ "incoming": incoming, "outgoing": [] }).to_string(),
        );
    }

    // Contact accept.
    if method == "POST" && path.starts_with("/contacts/") && path.ends_with("/accept") {
        let encoded = &path["/contacts/".len()..path.len() - "/accept".len()];
        let id = percent_decode(encoded);
        state.accepted.lock().unwrap().push(id.clone());
        // Drop it from the pending set so it is not re-offered.
        state
            .config
            .lock()
            .unwrap()
            .pending
            .retain(|p| p.agent_id != id);
        let contact = json!({ "requester": id, "addressee": "@self", "status": "accepted" });
        return ("200 OK", contact.to_string());
    }

    // Messages list.
    if method == "GET" && path == "/messages" {
        let messages = state.config.lock().unwrap().messages.clone();
        let list: Vec<Value> = messages
            .iter()
            .map(|m| json!({ "id": m.id, "from": m.from, "to": m.to, "body": m.body }))
            .collect();
        return ("200 OK", json!({ "messages": list }).to_string());
    }

    // Message acknowledge (destructive read).
    if method == "DELETE" && path.starts_with("/messages/") {
        let encoded = &path["/messages/".len()..];
        let id = percent_decode(encoded);
        state.acknowledged.lock().unwrap().push(id.clone());
        state.config.lock().unwrap().messages.retain(|m| m.id != id);
        return ("200 OK", String::new());
    }

    let _ = query;
    ("404 Not Found", json!({ "error": "not found" }).to_string())
}

/// The body of the most recent request (used to read the presence query's ids).
fn state_last_body(state: &Arc<MockState>) -> String {
    state
        .requests
        .lock()
        .unwrap()
        .last()
        .map(|r| r.body.clone())
        .unwrap_or_default()
}

fn requested_crypto_ids(body: &str) -> Vec<String> {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    value
        .get("cryptoIds")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn status_text(code: u16) -> &'static str {
    match code {
        500 => "500 Internal Server Error",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// Minimal percent-decoding for the `%XX` sequences the SDK emits for ids.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

async fn respond(sock: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(response.as_bytes()).await?;
    sock.flush().await?;
    let _ = sock.shutdown().await;
    Ok(())
}

/// Read one full HTTP request: request line, headers, and any `Content-Length`
/// body. Returns `(method, path, body)`.
async fn read_request(sock: &mut TcpStream) -> std::io::Result<Option<(String, String, String)>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end;
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        if buf.len() > 1_048_576 {
            return Ok(None);
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    while buf.len() < header_end + content_length {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body =
        String::from_utf8_lossy(&buf[header_end..(header_end + content_length).min(buf.len())])
            .to_string();

    Ok(Some((method, path, body)))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
