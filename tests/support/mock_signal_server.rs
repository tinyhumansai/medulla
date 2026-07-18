//! A mock tiny.place **Signal server**: the server side of the end-to-end
//! encrypted flows the vendored `tinyplace` SDK drives from the medulla runtime
//! ([`medulla::daemon::transport::SignalTransport`], the wrapper bridge, and the
//! `runtime` mailbox/contact/presence loops).
//!
//! It is a hand-rolled tokio [`TcpListener`] HTTP/1.1 server (one request per
//! connection, `Connection: close`), in the same style as
//! `tests/support/mock_tinyplace.rs` and `tests/support/mock_harness_relay.rs`.
//! The CRYPTO IS LIVE: this server only stores and relays opaque material —
//! published pre-key bundles and encrypted envelopes — and never sees, produces,
//! or verifies plaintext. Real X3DH + double-ratchet runs inside the SDK on both
//! ends; only the transport server is mocked.
//!
//! # Endpoints (auth headers accepted and ignored)
//!
//! Pre-key bundles (registration = publishing a bundle; the identity that lets
//! peers open an encrypted channel):
//! - `PUT  /keys/:id/signed-prekey`  body `{identityKey, signedPreKey}`      → `null`.
//!   Stores the agent's X25519 identity key + signed pre-key. Registration.
//! - `PUT  /keys/:id/prekeys`        body `{identityKey, preKeys:[...]}`      → `null`.
//!   Appends one-time pre-keys to the agent's supply.
//! - `GET  /keys/:id/bundle`         → `KeyBundle`
//!   `{agentId, identityKey, signedPreKey, oneTimePreKey|null, updatedAt}`.
//!   Pops one one-time pre-key per fetch. 404 when the agent has no bundle
//!   (or when a `drop_next_bundle` fault is armed). A `corrupt_next_bundle`
//!   fault tampers the signed-pre-key signature so the initiator rejects it.
//! - `GET  /keys/:id/health`         → `KeyHealth`
//!   `{agentId, oneTimePreKeyCount, lowOneTimePreKeys, updatedAt}`.
//!
//! Mailbox relay (opaque encrypted envelopes, queued per recipient):
//! - `PUT    /messages`              body `MessageEnvelope`                   → the
//!   stored envelope (server assigns `id`). The `body` field is base64
//!   ciphertext; the server records it verbatim for ciphertext assertions.
//! - `GET    /messages?agentId=..`   → `{messages:[MessageEnvelope,...]}`
//!   addressed to `agentId`. Fault knobs reshape this response: `fail_list`
//!   returns 5xx for the next N calls; `duplicate_delivery` returns each queued
//!   envelope twice (same id); `out_of_order` reverses the queue order.
//! - `DELETE /messages/:id?agentId=..` → `null`. Acknowledge (destructive read):
//!   removes the envelope from the queue so it never redelivers.
//!
//! Contacts (pure REST, no ratchet; not enforced for messaging, matching the
//! existing test relays):
//! - `GET  /contacts/requests`       → `{incoming:[{cryptoId,status,direction}],outgoing:[]}`.
//! - `POST /contacts/:id/accept`     → `Contact{requester,addressee,status}` and
//!   records the accepted id, dropping it from the pending set.
//!
//! Presence (pure REST):
//! - `POST /presence/heartbeat`      → `PresenceStatus{cryptoId,online}` (or a
//!   scripted 5xx). Increments a heartbeat counter.
//! - `POST /presence/query`          body `{cryptoIds:[...]}` → `{presence:[{cryptoId,online}]}`.
//!
//! Any other route → 404 `{error:"not found"}`.
//!
//! # State model
//!
//! - `bundles: agentId → {identity_key, signed_pre_key, one_time:[...]}` — the
//!   published key material; `GET bundle` pops one one-time pre-key.
//! - `queue: [envelope,...]` — a single ordered list of opaque envelopes; reads
//!   filter by recipient (`to`), writes assign `m<N>` ids, acks remove by id.
//! - `stored_bodies: [envelope,...]` — an append-only log of every envelope ever
//!   PUT, retained across acks so ciphertext assertions can inspect the full
//!   history (the live `queue` is drained by acks).
//! - `pending_contacts`, `accepted`, `online`, plus request/heartbeat counters.
//!
//! # Fault injection ([`SignalServerControls`])
//!
//! - `drop_next_bundle(n)` — the next `n` `GET bundle` calls 404 (bundle dropped).
//! - `corrupt_next_bundle()` — the next `GET bundle` serves a tampered signature.
//! - `fail_list(n)` — the next `n` `GET /messages` calls return 500.
//! - `set_duplicate_delivery(on)` — `GET /messages` returns each envelope twice.
//! - `set_out_of_order(on)` — `GET /messages` returns the queue reversed.
//! - `heartbeat_status(code)` — the presence heartbeat returns `code`.
//! - `add_pending_contact(id)` / `set_online(ids)` — seed contacts/presence.
//!
//! # Ciphertext guarantee
//!
//! [`MockSignalServer::assert_ciphertext_only`] panics unless, for every stored
//! envelope and every plaintext marker, the marker appears nowhere in the raw
//! envelope JSON and the base64-decoded `body` bytes are either not valid UTF-8
//! or do not contain the marker — proving the server only ever saw ciphertext.
//!
//! # e2e scenario matrix (driven from `tests/e2e_signal.rs`)
//!
//! 1. Two identities register (publish bundles), exchange bundles, round-trip
//!    encrypted DMs; server payloads asserted ciphertext-only; plus presence
//!    heartbeat/query and contact-accept against this server.
//! 2. Owner → daemon task chain: an owner sends a `medulla-tinyplace/1` task
//!    frame; a `DaemonRuntime` receives it over this server, runs it on a mock
//!    harness CLI, and the owner receives ack → status → reply, all encrypted.
//! 3. Same chain against the real `opencode` binary when on PATH (skipped, with a
//!    stderr note, when absent) — a terminal frame (reply or error) must arrive.
//! 4. Wrapper leg: the wrapper bridges a mock-harness session through this server
//!    to an owner; session envelopes arrive encrypted, decrypt to valid v2, and
//!    an inbound control frame reaches the child.
//! 5. Fault matrix: corrupt-bundle self-heal, 5xx-on-list retry, duplicate/
//!    out-of-order delivery, taskKey dedupe, ack drains the queue.
//! 6. medulla-API leg: decrypted frames fold into the expected agents-lane/task
//!    states via `medulla::ui::agents`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One published bundle's material for an agent.
#[derive(Default)]
struct AgentKeys {
    identity_key: String,
    signed_pre_key: Option<Value>,
    one_time: Vec<Value>,
}

/// A pending incoming contact request advertised on `GET /contacts/requests`.
#[derive(Clone)]
struct PendingContact {
    agent_id: String,
    status: String,
}

struct ServerState {
    bundles: Mutex<HashMap<String, AgentKeys>>,
    /// Live queue drained by acks.
    queue: Mutex<Vec<Value>>,
    /// Append-only log of every envelope ever PUT (kept for ciphertext asserts).
    stored: Mutex<Vec<Value>>,
    pending_contacts: Mutex<Vec<PendingContact>>,
    accepted: Mutex<Vec<String>>,
    online: Mutex<Vec<String>>,
    next_id: AtomicU64,
    // counters
    heartbeats: AtomicU32,
    presence_queries: AtomicU32,
    bundle_fetches: AtomicU32,
    list_calls: AtomicU32,
    sends: AtomicU32,
    acks: AtomicU32,
    // fault knobs
    corrupt_next_bundle: AtomicBool,
    drop_bundle_remaining: AtomicUsize,
    fail_list_remaining: AtomicUsize,
    duplicate_delivery: AtomicBool,
    out_of_order: AtomicBool,
    heartbeat_status: AtomicU32,
}

/// Runtime fault-injection + seeding knobs for a running server.
#[derive(Clone)]
pub struct SignalServerControls {
    state: Arc<ServerState>,
}

impl SignalServerControls {
    /// Tamper the signature on the *next* served bundle (drives session self-heal).
    pub fn corrupt_next_bundle(&self) {
        self.state.corrupt_next_bundle.store(true, Ordering::SeqCst);
    }

    /// Make the next `n` `GET /keys/:id/bundle` calls 404 (bundle dropped).
    pub fn drop_next_bundle(&self, n: usize) {
        self.state.drop_bundle_remaining.store(n, Ordering::SeqCst);
    }

    /// Make the next `n` `GET /messages` calls respond 500.
    pub fn fail_list(&self, n: usize) {
        self.state.fail_list_remaining.store(n, Ordering::SeqCst);
    }

    /// When on, `GET /messages` returns each queued envelope twice (same id).
    pub fn set_duplicate_delivery(&self, on: bool) {
        self.state.duplicate_delivery.store(on, Ordering::SeqCst);
    }

    /// When on, `GET /messages` returns the recipient's queue in reverse order.
    pub fn set_out_of_order(&self, on: bool) {
        self.state.out_of_order.store(on, Ordering::SeqCst);
    }

    /// HTTP status the presence heartbeat returns (200 by default).
    pub fn heartbeat_status(&self, code: u16) {
        self.state
            .heartbeat_status
            .store(code as u32, Ordering::SeqCst);
    }

    /// Seed a pending incoming contact request the auto-accepter will accept.
    pub fn add_pending_contact(&self, agent_id: &str) {
        self.state
            .pending_contacts
            .lock()
            .unwrap()
            .push(PendingContact {
                agent_id: agent_id.to_string(),
                status: "pending".to_string(),
            });
    }

    /// Report these ids `online:true` on `POST /presence/query`.
    pub fn set_online(&self, ids: &[String]) {
        *self.state.online.lock().unwrap() = ids.to_vec();
    }

    // ── introspection ────────────────────────────────────────────────────────

    /// Count of envelopes currently queued (undrained).
    pub fn queued_messages(&self) -> usize {
        self.state.queue.lock().unwrap().len()
    }

    /// Count of envelopes queued for a specific recipient.
    pub fn queued_for(&self, agent_id: &str) -> usize {
        self.state
            .queue
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.get("to").and_then(Value::as_str) == Some(agent_id))
            .count()
    }

    /// Every envelope ever PUT (append-only; survives acks).
    pub fn stored_envelopes(&self) -> Vec<Value> {
        self.state.stored.lock().unwrap().clone()
    }

    pub fn heartbeats(&self) -> u32 {
        self.state.heartbeats.load(Ordering::SeqCst)
    }
    pub fn presence_queries(&self) -> u32 {
        self.state.presence_queries.load(Ordering::SeqCst)
    }
    pub fn bundle_fetches(&self) -> u32 {
        self.state.bundle_fetches.load(Ordering::SeqCst)
    }
    pub fn list_calls(&self) -> u32 {
        self.state.list_calls.load(Ordering::SeqCst)
    }
    pub fn sends(&self) -> u32 {
        self.state.sends.load(Ordering::SeqCst)
    }
    pub fn acks(&self) -> u32 {
        self.state.acks.load(Ordering::SeqCst)
    }
    pub fn accepted(&self) -> Vec<String> {
        self.state.accepted.lock().unwrap().clone()
    }

    /// Inject a raw (attacker-shaped) envelope addressed to `to` — used to drive
    /// the decrypt-failure path with an undecryptable body.
    pub fn inject_raw(&self, from: &str, to: &str, body: &str) {
        let id = self.state.next_id.fetch_add(1, Ordering::SeqCst);
        let env = json!({
            "id": format!("m{id}"),
            "from": from,
            "to": to,
            "timestamp": "",
            "deviceId": 0,
            "type": "CIPHERTEXT",
            "body": body,
        });
        self.state.stored.lock().unwrap().push(env.clone());
        self.state.queue.lock().unwrap().push(env);
    }
}

/// A running mock Signal server. Drop it to stop the acceptor.
pub struct MockSignalServer {
    pub base_url: String,
    state: Arc<ServerState>,
    _accept: JoinHandle<()>,
}

impl Drop for MockSignalServer {
    fn drop(&mut self) {
        self._accept.abort();
    }
}

impl MockSignalServer {
    /// Bind on an ephemeral loopback port and start accepting.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(ServerState {
            bundles: Mutex::new(HashMap::new()),
            queue: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
            pending_contacts: Mutex::new(Vec::new()),
            accepted: Mutex::new(Vec::new()),
            online: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            heartbeats: AtomicU32::new(0),
            presence_queries: AtomicU32::new(0),
            bundle_fetches: AtomicU32::new(0),
            list_calls: AtomicU32::new(0),
            sends: AtomicU32::new(0),
            acks: AtomicU32::new(0),
            corrupt_next_bundle: AtomicBool::new(false),
            drop_bundle_remaining: AtomicUsize::new(0),
            fail_list_remaining: AtomicUsize::new(0),
            duplicate_delivery: AtomicBool::new(false),
            out_of_order: AtomicBool::new(false),
            heartbeat_status: AtomicU32::new(200),
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
        MockSignalServer {
            base_url: format!("http://{addr}"),
            state,
            _accept: accept,
        }
    }

    /// Fault-injection + introspection handle.
    pub fn controls(&self) -> SignalServerControls {
        SignalServerControls {
            state: self.state.clone(),
        }
    }

    /// Assert the server never saw any of `markers` in plaintext: for every
    /// stored envelope and marker, the marker must appear neither in the raw
    /// envelope JSON nor in the base64-decoded `body` bytes (which must be
    /// non-UTF-8 or marker-free). Panics on the first leak.
    pub fn assert_ciphertext_only(&self, markers: &[&str]) {
        let stored = self.state.stored.lock().unwrap();
        assert!(
            !stored.is_empty(),
            "assert_ciphertext_only called with no stored envelopes"
        );
        for env in stored.iter() {
            let raw = env.to_string();
            for marker in markers {
                assert!(
                    !raw.contains(marker),
                    "plaintext marker {marker:?} leaked into stored envelope JSON: {raw}"
                );
                if let Some(body) = env.get("body").and_then(Value::as_str) {
                    // The stored body string is base64 ciphertext; it must not
                    // contain the marker verbatim.
                    assert!(
                        !body.contains(marker),
                        "plaintext marker {marker:?} present in raw body string: {body}"
                    );
                    // Decoded bytes must be non-UTF-8 or, if UTF-8, marker-free.
                    if let Ok(bytes) = BASE64.decode(body.as_bytes()) {
                        if let Ok(text) = String::from_utf8(bytes) {
                            assert!(
                                !text.contains(marker),
                                "plaintext marker {marker:?} decoded out of ciphertext body: {text}"
                            );
                        }
                    }
                }
            }
        }
    }
}

async fn handle_conn(mut sock: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    let Some((method, raw_path, body)) = read_request(&mut sock).await? else {
        return Ok(());
    };
    let (path, query) = match raw_path.split_once('?') {
        Some((r, q)) => (r.to_string(), q.to_string()),
        None => (raw_path.clone(), String::new()),
    };
    let (status, response_body) = route(&method, &path, &query, &body, &state);
    respond(&mut sock, status, &response_body).await
}

fn route(
    method: &str,
    route: &str,
    query: &str,
    body: &str,
    state: &Arc<ServerState>,
) -> (&'static str, String) {
    // GET /keys/:id/bundle
    if method == "GET" && route.starts_with("/keys/") && route.ends_with("/bundle") {
        state.bundle_fetches.fetch_add(1, Ordering::SeqCst);
        let id = key_agent_id(route, "/bundle");
        if state.drop_bundle_remaining.load(Ordering::SeqCst) > 0 {
            state.drop_bundle_remaining.fetch_sub(1, Ordering::SeqCst);
            return ("404 Not Found", r#"{"error":"no bundle"}"#.to_string());
        }
        return match build_bundle(state, &id) {
            Some(bundle) => ("200 OK", bundle.to_string()),
            None => ("404 Not Found", r#"{"error":"no bundle"}"#.to_string()),
        };
    }
    // GET /keys/:id/health
    if method == "GET" && route.starts_with("/keys/") && route.ends_with("/health") {
        let id = key_agent_id(route, "/health");
        let count = state
            .bundles
            .lock()
            .unwrap()
            .get(&id)
            .map(|k| k.one_time.len())
            .unwrap_or(0);
        let health = json!({
            "agentId": id,
            "oneTimePreKeyCount": count,
            "lowOneTimePreKeys": count < 5,
            "updatedAt": "",
        });
        return ("200 OK", health.to_string());
    }
    // PUT /keys/:id/signed-prekey  (registration: identity + signed pre-key)
    if method == "PUT" && route.starts_with("/keys/") && route.ends_with("/signed-prekey") {
        let id = key_agent_id(route, "/signed-prekey");
        if let Ok(request) = serde_json::from_str::<Value>(body) {
            let mut keys = state.bundles.lock().unwrap();
            let entry = keys.entry(id).or_default();
            if let Some(ident) = request.get("identityKey").and_then(Value::as_str) {
                entry.identity_key = ident.to_string();
            }
            entry.signed_pre_key = request.get("signedPreKey").cloned();
        }
        return ("200 OK", "null".to_string());
    }
    // PUT /keys/:id/prekeys
    if method == "PUT" && route.starts_with("/keys/") && route.ends_with("/prekeys") {
        let id = key_agent_id(route, "/prekeys");
        if let Ok(request) = serde_json::from_str::<Value>(body) {
            let mut keys = state.bundles.lock().unwrap();
            let entry = keys.entry(id).or_default();
            if let Some(ident) = request.get("identityKey").and_then(Value::as_str) {
                entry.identity_key = ident.to_string();
            }
            if let Some(list) = request.get("preKeys").and_then(Value::as_array) {
                entry.one_time.extend(list.iter().cloned());
            }
        }
        return ("200 OK", "null".to_string());
    }
    // PUT /messages  (enqueue an opaque encrypted envelope)
    if method == "PUT" && route == "/messages" {
        if let Ok(mut envelope) = serde_json::from_str::<Value>(body) {
            state.sends.fetch_add(1, Ordering::SeqCst);
            let id = state.next_id.fetch_add(1, Ordering::SeqCst);
            envelope["id"] = json!(format!("m{id}"));
            state.stored.lock().unwrap().push(envelope.clone());
            state.queue.lock().unwrap().push(envelope.clone());
            return ("200 OK", envelope.to_string());
        }
        return ("400 Bad Request", r#"{"error":"bad envelope"}"#.to_string());
    }
    // GET /messages?agentId=..
    if method == "GET" && route == "/messages" {
        state.list_calls.fetch_add(1, Ordering::SeqCst);
        if state.fail_list_remaining.load(Ordering::SeqCst) > 0 {
            state.fail_list_remaining.fetch_sub(1, Ordering::SeqCst);
            return (
                "500 Internal Server Error",
                r#"{"error":"list unavailable"}"#.to_string(),
            );
        }
        let agent = query_param(query, "agentId");
        let mut messages: Vec<Value> = state
            .queue
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.get("to").and_then(Value::as_str) == Some(agent.as_str()))
            .cloned()
            .collect();
        if state.out_of_order.load(Ordering::SeqCst) {
            messages.reverse();
        }
        if state.duplicate_delivery.load(Ordering::SeqCst) {
            messages = messages
                .iter()
                .flat_map(|m| [m.clone(), m.clone()])
                .collect();
        }
        return ("200 OK", json!({ "messages": messages }).to_string());
    }
    // DELETE /messages/:id  (acknowledge/destructive read)
    if method == "DELETE" && route.starts_with("/messages/") {
        state.acks.fetch_add(1, Ordering::SeqCst);
        let id = percent_decode(route.trim_start_matches("/messages/"));
        state
            .queue
            .lock()
            .unwrap()
            .retain(|m| m.get("id").and_then(Value::as_str) != Some(id.as_str()));
        return ("200 OK", "null".to_string());
    }
    // POST /presence/heartbeat
    if method == "POST" && route == "/presence/heartbeat" {
        state.heartbeats.fetch_add(1, Ordering::SeqCst);
        let code = state.heartbeat_status.load(Ordering::SeqCst) as u16;
        if code >= 400 {
            return (
                status_text(code),
                r#"{"error":"heartbeat down"}"#.to_string(),
            );
        }
        return (
            "200 OK",
            json!({ "cryptoId": "@self", "online": true }).to_string(),
        );
    }
    // POST /presence/query
    if method == "POST" && route == "/presence/query" {
        state.presence_queries.fetch_add(1, Ordering::SeqCst);
        let online = state.online.lock().unwrap().clone();
        let requested = requested_crypto_ids(body);
        let presence: Vec<Value> = requested
            .iter()
            .map(|id| json!({ "cryptoId": id, "online": online.contains(id) }))
            .collect();
        return ("200 OK", json!({ "presence": presence }).to_string());
    }
    // GET /contacts/requests
    if method == "GET" && route == "/contacts/requests" {
        let pending = state.pending_contacts.lock().unwrap().clone();
        let incoming: Vec<Value> = pending
            .iter()
            .map(|p| json!({ "cryptoId": p.agent_id, "status": p.status, "direction": "incoming" }))
            .collect();
        return (
            "200 OK",
            json!({ "incoming": incoming, "outgoing": [] }).to_string(),
        );
    }
    // POST /contacts/:id/accept
    if method == "POST" && route.starts_with("/contacts/") && route.ends_with("/accept") {
        let encoded = &route["/contacts/".len()..route.len() - "/accept".len()];
        let id = percent_decode(encoded);
        state.accepted.lock().unwrap().push(id.clone());
        state
            .pending_contacts
            .lock()
            .unwrap()
            .retain(|p| p.agent_id != id);
        let contact = json!({ "requester": id, "addressee": "@self", "status": "accepted" });
        return ("200 OK", contact.to_string());
    }

    ("404 Not Found", r#"{"error":"not found"}"#.to_string())
}

/// Build a `KeyBundle` for `id`, popping one one-time pre-key. Applies the
/// signature-corruption fault when armed.
fn build_bundle(state: &Arc<ServerState>, id: &str) -> Option<Value> {
    let mut keys = state.bundles.lock().unwrap();
    let entry = keys.get_mut(id)?;
    let mut signed = entry.signed_pre_key.clone()?;
    let one_time = if entry.one_time.is_empty() {
        None
    } else {
        Some(entry.one_time.remove(0))
    };
    if state.corrupt_next_bundle.swap(false, Ordering::SeqCst) {
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
    percent_decode(route.trim_start_matches("/keys/").trim_end_matches(suffix))
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

fn query_param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return percent_decode(v);
            }
        }
    }
    String::new()
}

/// Minimal percent-decoding for the `%XX` sequences the SDK emits for ids.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
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

/// Read one full HTTP request: `(method, path, body)`.
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
