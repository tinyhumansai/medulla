//! Data types for the `boot` module.
#[allow(unused_imports)]
use super::*;
/// One worker the hub fronts on the backend roster.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// The `agentId` the backend targets (defaults to the tiny.place address).
    pub id: String,
    /// The worker's tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Display name for the roster entry.
    pub name: String,
    /// Free-text description / capability summary.
    pub description: String,
    /// The coding-agent harness the worker runs (`claude`/`codex`/`opencode`).
    pub harness: String,
}
/// Everything [`start_hub`] needs to bridge the backend to remote workers.
/// Not `Debug`: the log sink is a boxed closure with no useful representation,
/// and the JWT should not be printable by accident either.
#[derive(Clone)]
pub struct HubConfig {
    /// Backend Socket.IO base URL (e.g. `https://staging-api.tinyhumans.ai`).
    pub backend_url: String,
    /// JWT for the Socket.IO handshake (from `medulla login`).
    pub jwt: String,
    /// tiny.place identity directory (the hub's own wallet).
    pub identity_dir: PathBuf,
    /// The workers to advertise initially (may be empty; add more at runtime).
    pub workers: Vec<WorkerSpec>,
    /// How often the runner drains the encrypted inbox.
    pub poll: Duration,
    /// Where diagnostics go. Defaults to stderr; a TUI supplies its own so the
    /// hub never writes over a screen it does not own.
    pub log: super::super::types::HubLog,
    /// Where roster changes are saved. `None` keeps the roster in memory only.
    pub persist: Option<super::super::types::RosterSink>,
}
/// A running hub: the live [`HubHandle`] plus the client/runner kept alive for
/// the session (dropping this disconnects and stops the pump).
pub struct HubSession {
    /// Live roster control (add/remove/list workers), re-registering on change.
    pub handle: HubHandle,
    pub(super) _runner: Arc<TaskRunner>,
    pub(super) _client: Client,
}
