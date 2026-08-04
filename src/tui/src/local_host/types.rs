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
    pub(crate) fn providers(&self) -> &[medulla::protocol::HarnessProvider] {
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

    /// A clone of the host's task state machine.
    ///
    /// The UI needs exactly one thing from it —
    /// [`session_for_task`](medulla::daemon::DaemonRuntime::session_for_task),
    /// which answers "which live harness session is running the task the cursor
    /// is on". Without it the Agents tab could only guess by matching labels,
    /// and two sessions for one peer would make that guess wrong. Cheap to
    /// clone (an `Arc`), and a clone does *not* keep the host alive.
    pub(crate) fn runtime(&self) -> medulla::daemon::DaemonRuntime {
        self.daemon.runtime().clone()
    }
}

/// What Medulla imposes on every harness a host launches: commit attribution and
/// the operator's lifecycle hooks.
///
/// The two always travel together — both come from the same loaded config, and
/// on Claude Code they are delivered through the same `--settings` flag — so they
/// cross the host-options boundary as one value rather than as two positional
/// arguments that could be passed in the wrong order.
#[derive(Debug, Clone, Default)]
pub(crate) struct LaunchPolicy {
    /// Whether commits carry the `Co-authored-by: Medulla` trailer.
    pub(crate) attribution: bool,
    /// The resolved `[[hooks]]` config section.
    pub(crate) hooks: medulla::harness_hooks::HooksConfig,
}
