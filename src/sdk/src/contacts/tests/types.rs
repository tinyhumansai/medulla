//! Shared fixture types for contact admission tests.

#[allow(unused_imports)]
use super::*;

/// A relay that serves a fixed incoming list and records every call.
#[derive(Default)]
pub(super) struct FakeRelay {
    pub(super) incoming: Mutex<Vec<IncomingRequest>>,
    /// Peers the relay already considers contacts.
    pub(super) accepted: Mutex<Vec<IncomingRequest>>,
    pub(super) calls: Mutex<Vec<String>>,
    pub(super) fail: Mutex<bool>,
    /// Fails only the contact-list read, so a decision can succeed while the
    /// re-read that follows it does not.
    pub(super) fail_list: Mutex<bool>,
    /// Every relay interaction in order, listings included.
    pub(super) trace: Mutex<Vec<String>>,
}
