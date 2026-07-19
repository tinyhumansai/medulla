//! State model, fault-injection controls, and the server handle for the mock
//! tiny.place Signal server.
//!
//! Holds the published key material, the opaque envelope queue/append-log, and
//! the pending-contact/presence bookkeeping ([`ServerState`]), plus the two
//! public handles the tests drive: [`SignalServerControls`] (fault knobs +
//! introspection) and [`MockSignalServer`] (binds the listener, hands out
//! controls, and enforces the ciphertext-only guarantee). Request routing lives
//! in the sibling `routing` module; wire helpers live in `http`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::routing::handle_conn;

/// One published bundle's material for an agent.
#[derive(Default)]
pub struct AgentKeys {
    pub identity_key: String,
    pub signed_pre_key: Option<Value>,
    pub one_time: Vec<Value>,
}

/// A pending incoming contact request advertised on `GET /contacts/requests`.
#[derive(Clone)]
pub struct PendingContact {
    pub agent_id: String,
    pub status: String,
}

pub struct ServerState {
    pub bundles: Mutex<HashMap<String, AgentKeys>>,
    /// Live queue drained by acks.
    pub queue: Mutex<Vec<Value>>,
    /// Append-only log of every envelope ever PUT (kept for ciphertext asserts).
    pub stored: Mutex<Vec<Value>>,
    pub pending_contacts: Mutex<Vec<PendingContact>>,
    pub accepted: Mutex<Vec<String>>,
    pub online: Mutex<Vec<String>>,
    pub next_id: AtomicU64,
    // counters
    pub heartbeats: AtomicU32,
    pub presence_queries: AtomicU32,
    pub bundle_fetches: AtomicU32,
    pub list_calls: AtomicU32,
    pub sends: AtomicU32,
    pub acks: AtomicU32,
    // fault knobs
    pub corrupt_next_bundle: AtomicBool,
    pub drop_bundle_remaining: AtomicUsize,
    pub fail_list_remaining: AtomicUsize,
    pub duplicate_delivery: AtomicBool,
    pub out_of_order: AtomicBool,
    pub heartbeat_status: AtomicU32,
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
        Self::start_on_addr("127.0.0.1:0").await
    }

    /// Bind on a specific address (e.g. `"127.0.0.1:8787"` for a runnable server)
    /// and start accepting. `start()` calls this with an ephemeral loopback port.
    pub async fn start_on_addr(addr: &str) -> Self {
        let listener = TcpListener::bind(addr).await.unwrap();
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
