//! An in-memory tiny.place relay mock, just enough of the REST surface for the
//! daemon's [`medulla::daemon::transport::SignalTransport`] to run encrypted DM
//! round-trips with no network and no real backend.
//!
//! It stores per-agent published key material and a shared message queue, so two
//! `SignalTransport`s pointed at the same relay can `publish_keys`, `send`, and
//! `drain_inbox` against each other — exercising X3DH bundle exchange, the
//! double-ratchet encrypt/decrypt path, and the acknowledge/delete semantics.
//!
//! Endpoints (auth headers are accepted and ignored):
//! - `PUT /keys/:id/signed-prekey`  → store identity key + signed pre-key
//! - `PUT /keys/:id/prekeys`        → store one-time pre-keys
//! - `GET /keys/:id/bundle`         → a `KeyBundle` (pops one one-time pre-key)
//! - `GET /keys/:id/health`         → a `KeyHealth`
//! - `PUT /messages`               → enqueue an envelope (assigns an id)
//! - `GET /messages?agentId=..`    → `{ "messages": [...] }` addressed to the id
//! - `DELETE /messages/:id`        → remove the enqueued envelope
//!
//! Fault injection: [`RelayControls::corrupt_next_bundle`] tampers the next
//! served signed-pre-key signature (drives the session self-heal retry), and
//! [`RelayControls::fail_list`] makes `GET /messages` 500 (drives the
//! drain-inbox error path).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Default)]
struct AgentKeys {
    identity_key: String,
    signed_pre_key: Option<Value>,
    one_time: Vec<Value>,
}

struct RelayState {
    keys: Mutex<HashMap<String, AgentKeys>>,
    messages: Mutex<Vec<Value>>,
    next_id: AtomicU64,
    corrupt_next_bundle: AtomicBool,
    fail_list: AtomicBool,
}

/// Runtime fault-injection knobs for a running relay.
#[derive(Clone)]
pub struct RelayControls {
    state: Arc<RelayState>,
}

impl RelayControls {
    /// Tamper the signature on the *next* served bundle so the initiating side
    /// rejects it as a bad signed pre-key (a session-shaped error).
    pub fn corrupt_next_bundle(&self) {
        self.state.corrupt_next_bundle.store(true, Ordering::SeqCst);
    }

    /// Make the next `GET /messages` respond 500.
    pub fn fail_list(&self, on: bool) {
        self.state.fail_list.store(on, Ordering::SeqCst);
    }

    /// How many envelopes are currently queued.
    pub fn queued_messages(&self) -> usize {
        self.state.messages.lock().unwrap().len()
    }

    /// How many one-time pre-keys the relay currently holds for `agent_id` — the
    /// same pool `GET /health` counts. Lets a test assert that an idempotent
    /// `publish_keys` did NOT re-upload a fresh batch.
    pub fn one_time_count(&self, agent_id: &str) -> usize {
        self.state
            .keys
            .lock()
            .unwrap()
            .get(agent_id)
            .map(|k| k.one_time.len())
            .unwrap_or(0)
    }

    /// Empty `agent_id`'s one-time pre-key pool, simulating every key being
    /// consumed by peers' handshakes — so a subsequent health check reports the
    /// pool depleted and `publish_keys` must top it back up.
    pub fn drain_one_time(&self, agent_id: &str) {
        if let Some(k) = self.state.keys.lock().unwrap().get_mut(agent_id) {
            k.one_time.clear();
        }
    }

    /// Inject a raw envelope addressed to `to` (used to exercise the decrypt
    /// failure path with an undecryptable body).
    pub fn inject_message(&self, from: &str, to: &str, body: &str) {
        let id = self.state.next_id.fetch_add(1, Ordering::SeqCst);
        self.state.messages.lock().unwrap().push(json!({
            "id": format!("m{id}"),
            "from": from,
            "to": to,
            "timestamp": "",
            "deviceId": 0,
            "type": "CIPHERTEXT",
            "body": body,
        }));
    }
}

/// A running relay. Drop it to stop the acceptor.
pub struct MockRelay {
    pub base_url: String,
    state: Arc<RelayState>,
    _accept: JoinHandle<()>,
}

impl Drop for MockRelay {
    fn drop(&mut self) {
        self._accept.abort();
    }
}

impl MockRelay {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(RelayState {
            keys: Mutex::new(HashMap::new()),
            messages: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            corrupt_next_bundle: AtomicBool::new(false),
            fail_list: AtomicBool::new(false),
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
        MockRelay {
            base_url: format!("http://{addr}"),
            state,
            _accept: accept,
        }
    }

    pub fn controls(&self) -> RelayControls {
        RelayControls {
            state: self.state.clone(),
        }
    }
}

async fn handle_conn(mut sock: TcpStream, state: Arc<RelayState>) -> std::io::Result<()> {
    let Some((method, path, body)) = read_request(&mut sock).await? else {
        return Ok(());
    };
    let (route, query) = match path.split_once('?') {
        Some((route, query)) => (route.to_string(), query.to_string()),
        None => (path.clone(), String::new()),
    };

    // GET /keys/:id/bundle
    if method == "GET" && route.starts_with("/keys/") && route.ends_with("/bundle") {
        let id = key_agent_id(&route, "/bundle");
        return match build_bundle(&state, &id) {
            Some(bundle) => respond(&mut sock, "200 OK", &bundle.to_string()).await,
            None => respond(&mut sock, "404 Not Found", r#"{"error":"no bundle"}"#).await,
        };
    }
    // GET /keys/:id/health
    if method == "GET" && route.starts_with("/keys/") && route.ends_with("/health") {
        let id = key_agent_id(&route, "/health");
        let (count, signed_pre_key_id) = {
            let keys = state.keys.lock().unwrap();
            match keys.get(&id) {
                Some(k) => (
                    k.one_time.len(),
                    k.signed_pre_key
                        .as_ref()
                        .and_then(|spk| spk.get("keyId").cloned()),
                ),
                None => (0, None),
            }
        };
        let health = json!({
            "agentId": id,
            "oneTimePreKeyCount": count,
            "lowOneTimePreKeys": count < 5,
            // The real relay reports WHICH signed pre-key it is serving; key
            // maintenance uses it to confirm the store can still back that exact
            // id before deciding a rotation is unnecessary.
            "signedPreKeyKeyId": signed_pre_key_id,
            "updatedAt": "",
        });
        return respond(&mut sock, "200 OK", &health.to_string()).await;
    }
    // PUT /keys/:id/signed-prekey
    if method == "PUT" && route.starts_with("/keys/") && route.ends_with("/signed-prekey") {
        let id = key_agent_id(&route, "/signed-prekey");
        if let Ok(request) = serde_json::from_str::<Value>(&body) {
            let mut keys = state.keys.lock().unwrap();
            let entry = keys.entry(id).or_default();
            if let Some(ident) = request.get("identityKey").and_then(Value::as_str) {
                entry.identity_key = ident.to_string();
            }
            entry.signed_pre_key = request.get("signedPreKey").cloned();
        }
        return respond(&mut sock, "200 OK", "null").await;
    }
    // PUT /keys/:id/prekeys
    if method == "PUT" && route.starts_with("/keys/") && route.ends_with("/prekeys") {
        let id = key_agent_id(&route, "/prekeys");
        if let Ok(request) = serde_json::from_str::<Value>(&body) {
            let mut keys = state.keys.lock().unwrap();
            let entry = keys.entry(id).or_default();
            if let Some(ident) = request.get("identityKey").and_then(Value::as_str) {
                entry.identity_key = ident.to_string();
            }
            if let Some(list) = request.get("preKeys").and_then(Value::as_array) {
                entry.one_time.extend(list.iter().cloned());
            }
        }
        return respond(&mut sock, "200 OK", "null").await;
    }
    // PUT /messages
    if method == "PUT" && route == "/messages" {
        if let Ok(mut envelope) = serde_json::from_str::<Value>(&body) {
            let id = state.next_id.fetch_add(1, Ordering::SeqCst);
            envelope["id"] = json!(format!("m{id}"));
            state.messages.lock().unwrap().push(envelope.clone());
            return respond(&mut sock, "200 OK", &envelope.to_string()).await;
        }
        return respond(&mut sock, "400 Bad Request", r#"{"error":"bad envelope"}"#).await;
    }
    // GET /messages?agentId=..
    if method == "GET" && route == "/messages" {
        if state.fail_list.load(Ordering::SeqCst) {
            return respond(
                &mut sock,
                "500 Internal Server Error",
                r#"{"error":"boom"}"#,
            )
            .await;
        }
        let agent = query_param(&query, "agentId");
        let messages: Vec<Value> = state
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.get("to").and_then(Value::as_str) == Some(agent.as_str()))
            .cloned()
            .collect();
        let payload = json!({ "messages": messages });
        return respond(&mut sock, "200 OK", &payload.to_string()).await;
    }
    // DELETE /messages/:id
    if method == "DELETE" && route.starts_with("/messages/") {
        let id = route.trim_start_matches("/messages/").to_string();
        state
            .messages
            .lock()
            .unwrap()
            .retain(|m| m.get("id").and_then(Value::as_str) != Some(id.as_str()));
        return respond(&mut sock, "200 OK", "null").await;
    }

    respond(&mut sock, "404 Not Found", r#"{"error":"not found"}"#).await
}

/// Build a `KeyBundle` for `id`, popping one one-time pre-key. Applies the
/// signature-corruption fault when armed.
fn build_bundle(state: &Arc<RelayState>, id: &str) -> Option<Value> {
    let mut keys = state.keys.lock().unwrap();
    let entry = keys.get_mut(id)?;
    let mut signed = entry.signed_pre_key.clone()?;
    let one_time = if entry.one_time.is_empty() {
        None
    } else {
        Some(entry.one_time.remove(0))
    };
    if state.corrupt_next_bundle.swap(false, Ordering::SeqCst) {
        // Flip the signed pre-key's signature to an invalid (but well-formed)
        // value so the initiator rejects it as a bad signed pre-key.
        signed["signature"] = json!("AAAA");
    }
    Some(json!({
        "agentId": id,
        "identityKey": entry.identity_key,
        "signedPreKey": signed,
        "oneTimePreKey": one_time,
        "updatedAt": "",
    }))
}

fn key_agent_id(route: &str, suffix: &str) -> String {
    route
        .trim_start_matches("/keys/")
        .trim_end_matches(suffix)
        .to_string()
}

fn query_param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return decode_component(v);
            }
        }
    }
    String::new()
}

fn decode_component(value: &str) -> String {
    // Minimal percent-decoding for the agentId query param.
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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

/// Read one HTTP request: `(method, path, body)`.
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
    haystack.windows(needle.len()).position(|w| w == needle)
}
