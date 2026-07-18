//! A minimal in-test mock of the Medulla backend HTTP + SSE API.
//!
//! Speaks just enough HTTP/1.1 (one request per connection, `Connection: close`)
//! to satisfy [`medulla::client::MedullaClient`]:
//!
//! - `POST /medulla/v1/sessions`            → 201 `{sessionId}`
//! - `GET  /medulla/v1/sessions`            → 200 session list
//! - `GET  /medulla/v1/sessions/:id`        → 200 session detail (`eventSeq`)
//! - `POST /medulla/v1/sessions/:id/messages` → 202 `{cycleId,seq}` (or 500)
//! - `GET  /medulla/v1/sessions/:id/messages` → 200 message replay
//! - `POST /medulla/v1/sessions/:id/abort`  → 200 `{aborted:true}`
//! - `GET  /medulla/v1/sessions/:id/stream` → SSE, scripted per test
//!
//! The SSE body is driven live: tests call [`MockBackend::emit`] / `emit_ping`
//! to append frames the currently-connected stream writes out, and
//! [`MockBackend::close_stream`] to drop the active connection (exercising the
//! client's reconnect + `Last-Event-ID` replay). Every request is recorded for
//! assertions.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// One recorded inbound HTTP request.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub body: String,
    pub last_event_id: Option<String>,
}

/// Tunable response behaviour for the mock.
#[derive(Clone)]
pub struct MockConfig {
    pub created_session_id: String,
    pub messages_ok: bool,
    pub sessions_list: Value,
    pub messages_replay: Value,
    pub session_event_seq: i64,
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            created_session_id: "sess-1".to_string(),
            messages_ok: true,
            sessions_list: json!([]),
            messages_replay: json!([]),
            session_event_seq: 0,
        }
    }
}

struct MockState {
    requests: Mutex<Vec<RecordedRequest>>,
    config: Mutex<MockConfig>,
    sse_log: Mutex<Vec<String>>,
    append: broadcast::Sender<()>,
    close: broadcast::Sender<()>,
    stream_conns: AtomicUsize,
}

/// A running mock backend. Drop it to stop the acceptor.
pub struct MockBackend {
    pub base_url: String,
    state: Arc<MockState>,
    _accept: JoinHandle<()>,
}

impl Drop for MockBackend {
    fn drop(&mut self) {
        self._accept.abort();
    }
}

impl MockBackend {
    /// Bind on an ephemeral port and start accepting.
    pub async fn start() -> Self {
        Self::start_with(MockConfig::default()).await
    }

    /// Bind with a specific initial [`MockConfig`].
    pub async fn start_with(config: MockConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (append, _) = broadcast::channel(256);
        let (close, _) = broadcast::channel(16);
        let state = Arc::new(MockState {
            requests: Mutex::new(Vec::new()),
            config: Mutex::new(config),
            sse_log: Mutex::new(Vec::new()),
            append,
            close,
            stream_conns: AtomicUsize::new(0),
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
        MockBackend {
            base_url: format!("http://{addr}"),
            state,
            _accept: accept,
        }
    }

    /// Mutate the config (e.g. flip `messages_ok`).
    pub fn configure(&self, f: impl FnOnce(&mut MockConfig)) {
        f(&mut self.state.config.lock().unwrap());
    }

    /// Append a persisted SSE event (`id:` + `data:`) the active stream flushes.
    pub fn emit(&self, seq: u64, session: &str, event: Value) {
        let envelope = json!({
            "seq": seq,
            "at": 1_700_000_000_000u64,
            "sessionId": session,
            "event": event,
        });
        let chunk = format!("id: {seq}\ndata: {envelope}\n\n");
        self.state.sse_log.lock().unwrap().push(chunk);
        let _ = self.state.append.send(());
    }

    /// Append a heartbeat comment frame.
    pub fn emit_ping(&self) {
        self.state.sse_log.lock().unwrap().push(": ping\n\n".to_string());
        let _ = self.state.append.send(());
    }

    /// Close the active SSE connection(s), forcing the client to reconnect.
    pub fn close_stream(&self) {
        let _ = self.state.close.send(());
    }

    /// Every request seen so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.requests.lock().unwrap().clone()
    }

    /// How many SSE stream connections have been opened.
    pub fn stream_connections(&self) -> usize {
        self.state.stream_conns.load(Ordering::SeqCst)
    }

    /// Poll recorded requests until one matches `pred` or `timeout` elapses.
    pub async fn wait_for_request(
        &self,
        timeout: Duration,
        pred: impl Fn(&RecordedRequest) -> bool,
    ) -> RecordedRequest {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = self.requests().into_iter().find(&pred) {
                return found;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for a matching request");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

async fn handle_conn(mut sock: TcpStream, state: Arc<MockState>) -> std::io::Result<()> {
    let Some((method, path, headers, body)) = read_request(&mut sock).await? else {
        return Ok(());
    };
    let last_event_id = headers
        .iter()
        .find(|(k, _)| k == "last-event-id")
        .map(|(_, v)| v.clone());
    state.requests.lock().unwrap().push(RecordedRequest {
        method: method.clone(),
        path: path.clone(),
        body,
        last_event_id,
    });

    let route_path = path.split('?').next().unwrap_or(&path).to_string();

    if method == "GET" && route_path.ends_with("/stream") {
        serve_stream(sock, state).await;
        return Ok(());
    }

    let config = state.config.lock().unwrap().clone();
    let (status, data): (&str, Value) = if route_path == "/medulla/v1/sessions" && method == "POST"
    {
        ("201 Created", json!({ "sessionId": config.created_session_id }))
    } else if route_path == "/medulla/v1/sessions" && method == "GET" {
        ("200 OK", config.sessions_list.clone())
    } else if route_path.ends_with("/messages") && method == "POST" {
        if config.messages_ok {
            ("202 Accepted", json!({ "cycleId": "cycle-1", "seq": 1 }))
        } else {
            return respond_raw(&mut sock, "500 Internal Server Error", r#"{"success":false,"error":"boom","errorCode":"SERVER_ERROR"}"#).await;
        }
    } else if route_path.ends_with("/messages") && method == "GET" {
        ("200 OK", config.messages_replay.clone())
    } else if route_path.ends_with("/abort") && method == "POST" {
        let session = session_id_from(&route_path);
        ("200 OK", json!({ "sessionId": session, "aborted": true }))
    } else if method == "GET" && route_path.starts_with("/medulla/v1/sessions/") {
        // Session detail (`/sessions/:id` with no further path segment).
        let session = session_id_from(&route_path);
        (
            "200 OK",
            json!({
                "sessionId": session,
                "status": "idle",
                "eventSeq": config.session_event_seq,
            }),
        )
    } else {
        ("404 Not Found", json!({ "error": "not found" }))
    };

    let envelope = json!({ "success": true, "data": data });
    respond_raw(&mut sock, status, &envelope.to_string()).await
}

/// The `:id` segment of `/medulla/v1/sessions/:id[/...]`.
fn session_id_from(path: &str) -> String {
    path.strip_prefix("/medulla/v1/sessions/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string()
}

async fn respond_raw(sock: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(response.as_bytes()).await?;
    sock.flush().await?;
    let _ = sock.shutdown().await;
    Ok(())
}

async fn serve_stream(mut sock: TcpStream, state: Arc<MockState>) {
    state.stream_conns.fetch_add(1, Ordering::SeqCst);
    let mut append_rx = state.append.subscribe();
    let mut close_rx = state.close.subscribe();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
    if sock.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let _ = sock.flush().await;

    let mut sent = 0usize;
    loop {
        let chunks: Vec<String> = {
            let log = state.sse_log.lock().unwrap();
            log[sent.min(log.len())..].to_vec()
        };
        for chunk in &chunks {
            if sock.write_all(chunk.as_bytes()).await.is_err() {
                return;
            }
        }
        sent += chunks.len();
        let _ = sock.flush().await;

        tokio::select! {
            _ = close_rx.recv() => {
                let _ = sock.shutdown().await;
                return;
            }
            _ = append_rx.recv() => {
                // New frame(s) appended (or a lagged notice) — loop and flush.
            }
        }
    }
}

/// Read one full HTTP request: request line, headers, and any `Content-Length`
/// body. Returns `(method, path, headers, body)`.
async fn read_request(
    sock: &mut TcpStream,
) -> std::io::Result<Option<(String, String, Vec<(String, String)>, String)>> {
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

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let value = v.trim().to_string();
            if key == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((key, value));
        }
    }

    while buf.len() < header_end + content_length {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..(header_end + content_length).min(buf.len())])
        .to_string();

    Ok(Some((method, path, headers, body)))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
