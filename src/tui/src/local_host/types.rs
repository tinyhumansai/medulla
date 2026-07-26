//! Data types for the `local_host` module.

#[allow(unused_imports)]
use super::*;

/// A host running inside the TUI process, with the bus it shares with the hub.
///
/// Held by the TUI for the life of the session. Dropping it stops the host — the
/// bus survives in the hub's relay, but nothing answers on the host's address,
/// which is the honest state of affairs.
#[derive(Debug)]
pub(crate) struct LocalHost {
    /// The running host. Kept private so the UI reads it through this type's
    /// accessors rather than reaching into the SDK handle.
    pub(super) daemon: EmbeddedDaemon,
    /// A roster entry naming this host, for the hub to advertise.
    pub(super) spec: WorkerSpec,
}

impl LocalHost {
    /// The device-local address the orchestrator dispatches to.
    pub(crate) fn address(&self) -> &str {
        self.daemon.address()
    }

    /// Where tasks this host accepts will run.
    pub(crate) fn workspace(&self) -> &str {
        self.daemon.workspace()
    }

    /// The coding-agent CLIs this machine actually has.
    pub(crate) fn providers(&self) -> &[medulla::tinyplace::HarnessProvider] {
        self.daemon.providers()
    }

    /// A cloneable read-only view for the UI, carrying both this host's identity
    /// and its live counters.
    ///
    /// Handed to each session rather than the host itself: a session is rebuilt
    /// on every relogin, and the host outlives all of them.
    pub(crate) fn observation(&self) -> medulla::daemon::embedded::HostObservation {
        self.daemon.observation()
    }

    /// The roster entry the hub should advertise for this host.
    pub(crate) fn spec(&self) -> &WorkerSpec {
        &self.spec
    }
}
