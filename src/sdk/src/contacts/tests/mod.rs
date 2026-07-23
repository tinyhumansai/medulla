//! Unit tests for contact-request admission, split by surface so no file
//! exceeds the repo's 500-line ceiling: [`policy`] covers policy evaluation and
//! the idempotent pending queue, [`service`] decision execution against a fake
//! relay, and [`health`] what a poll reports about itself.
//!
//! The fake relay and the stepping clock live here because two of the three
//! submodules drive them; nothing else does.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::book::ContactBook;
use super::desk::ContactDesk;
use super::service::{decide, poll_once, ContactRelay, IncomingRequest, NowFn};
use super::types::{AdmissionPolicy, ContactDecision, RequestState};

mod health;
mod policy;
mod service;

/// A clock that advances one step per read.
fn clock() -> NowFn {
    let counter = Arc::new(AtomicI64::new(100));
    Arc::new(move || counter.fetch_add(1, Ordering::SeqCst))
}

/// A relay that serves a fixed incoming list and records every call.
#[derive(Default)]
struct FakeRelay {
    incoming: Mutex<Vec<IncomingRequest>>,
    /// Peers the relay already considers contacts.
    accepted: Mutex<Vec<IncomingRequest>>,
    calls: Mutex<Vec<String>>,
    fail: Mutex<bool>,
    /// Fails only the contact-list read, so a decision can succeed while the
    /// re-read that follows it does not.
    fail_list: Mutex<bool>,
    /// Every relay interaction in order, listings included.
    trace: Mutex<Vec<String>>,
}

impl FakeRelay {
    fn with_incoming(ids: &[&str]) -> Arc<Self> {
        Arc::new(FakeRelay {
            incoming: Mutex::new(
                ids.iter()
                    .map(|id| IncomingRequest {
                        agent_id: (*id).to_string(),
                        handle: None,
                    })
                    .collect(),
            ),
            ..FakeRelay::default()
        })
    }

    /// A relay whose contact graph already holds `ids` — peers accepted before
    /// this process started, or from another device.
    fn with_contacts(ids: &[&str]) -> Arc<Self> {
        Arc::new(FakeRelay {
            accepted: Mutex::new(
                ids.iter()
                    .map(|id| IncomingRequest {
                        agent_id: (*id).to_string(),
                        handle: None,
                    })
                    .collect(),
            ),
            ..FakeRelay::default()
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn trace(&self) -> Vec<String> {
        self.trace.lock().unwrap().clone()
    }

    fn record(&self, call: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(call.to_string());
        self.trace.lock().unwrap().push(call.to_string());
        if *self.fail.lock().unwrap() {
            return Err("relay unreachable".to_string());
        }
        Ok(())
    }
}

impl ContactRelay for FakeRelay {
    fn incoming(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move {
            // A relay that is down cannot list either, so the fail flag covers
            // listing as well as decisions.
            if *self.fail.lock().unwrap() {
                return Err("relay unreachable".to_string());
            }
            Ok(self.incoming.lock().unwrap().clone())
        })
    }
    fn accepted(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move {
            // Traced (not recorded as a "call") so a test can tell "the list
            // was re-read" apart from "the local settle happened to leave the
            // right answer", without disturbing the decision-only assertions.
            self.trace.lock().unwrap().push("list".to_string());
            if *self.fail.lock().unwrap() || *self.fail_list.lock().unwrap() {
                return Err("relay unreachable".to_string());
            }
            Ok(self.accepted.lock().unwrap().clone())
        })
    }
    fn accept(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.record(&format!("accept:{agent_id}"))?;
            // Mirror the relay: accepting moves the peer out of the request
            // queue and into the contact list.
            self.incoming
                .lock()
                .unwrap()
                .retain(|request| request.agent_id != agent_id);
            self.accepted.lock().unwrap().push(IncomingRequest {
                agent_id,
                handle: None,
            });
            Ok(())
        })
    }
    fn decline(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.record(&format!("decline:{agent_id}")) })
    }
    fn block(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.record(&format!("block:{agent_id}")) })
    }
}
