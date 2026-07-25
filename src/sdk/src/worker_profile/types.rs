//! Data types for the `worker_profile` module.
#[allow(unused_imports)]
use super::*;
/// The persisted worker identity: what the operator named this worker, its
/// tiny.place wallet address, the OpenHuman owner it answers to, and when it was
/// first registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkerProfile {
    /// Operator-chosen worker name (defaults to [`default_worker_name`]).
    pub name: String,
    /// The tiny.place identity (wallet) address this worker registered with.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub address: String,
    /// The OpenHuman owner (`@handle` or address) this worker answers to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub owner: Option<String>,
    /// ISO-8601 timestamp of first registration.
    #[serde(
        rename = "registeredAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub registered_at: Option<String>,
}
