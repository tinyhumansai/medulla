//! Data types for the `helpers` module.

#[allow(unused_imports)]
use super::*;

pub(in super::super) struct FakeRelay {
    pub(super) incoming: Vec<IncomingRequest>,
    /// Peers the relay already holds as contacts, independent of any request.
    pub(super) contacts: Vec<IncomingRequest>,
}
