//! Data types for the `book` module.
#[allow(unused_imports)]
use super::*;
/// The observed pending queue plus the operator's decisions.
///
/// Cheap to clone (an `Arc`), so the poll loop and the UI share one.
#[derive(Clone)]
pub struct ContactBook {
    pub(super) inner: Arc<Mutex<Inner>>,
}
pub(super) struct Inner {
    pub(super) policy: AdmissionPolicy,
    pub(super) allowlist: HashSet<String>,
    /// Requests in first-seen order, so the list does not reshuffle under the
    /// operator's cursor as the poll loop runs.
    pub(super) requests: Vec<ContactRequest>,
}
