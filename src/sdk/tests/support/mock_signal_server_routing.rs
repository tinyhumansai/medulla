//! Request routing for the mock tiny.place Signal server.
//!
//! [`handle_conn`] reads one HTTP request off a connection (via the sibling
//! `http` module), dispatches it through [`route`], and writes the response.
//! `route` implements every endpoint documented on the crate root, reading and
//! mutating the [`ServerState`] owned by the `state` module and applying the
//! armed fault knobs. `build_bundle` assembles a `KeyBundle`, popping one
//! one-time pre-key and optionally corrupting the signature.

#![allow(dead_code)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpStream;

use super::http::{
    key_agent_id, percent_decode, query_param, read_request, requested_crypto_ids, respond,
    status_text,
};
use super::state::ServerState;

pub async fn handle_conn(mut sock: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
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
    // GET /debug/stored?to=..  (introspection-only, not part of the tiny.place
    // API): the append-only count of envelopes ever addressed to a recipient, so
    // a runnable e2e can assert delivery without decrypting. Never affects the
    // live queue or any counter the scenario tests observe.
    if method == "GET" && route == "/debug/stored" {
        let to = query_param(query, "to");
        let count = state
            .stored
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.get("to").and_then(Value::as_str) == Some(to.as_str()))
            .count();
        return ("200 OK", json!({ "to": to, "count": count }).to_string());
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
