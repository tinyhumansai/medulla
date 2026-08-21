//! The one embedded core this process runs, shared by everything that needs it.
//!
//! [`boot`](super::boot) builds a core. Until now exactly one caller did that
//! — the TUI, once, at startup — so "the core" and "the core the TUI booted"
//! were the same thing and nothing had to name the difference.
//!
//! A workflow `agent` node that names `harness: openhuman` breaks that. It runs
//! deep inside the engine, on whatever process happens to be executing the run:
//! the operator's TUI, a `medulla workflow run` on a terminal, or the `medulla
//! mcp` subprocess a harness session spawned. Only the first of those has a core
//! already, and none of them can be handed one down the call stack — the
//! dispatch signature belongs to every harness, not just this one.
//!
//! So the core becomes a process-wide resource with two doors:
//!
//! * [`install`] — a host that boots its own core donates it, so a node running
//!   inside the TUI uses the very core the operator is looking at rather than a
//!   second one beside it.
//! * [`shared`] — anything that needs a core takes the installed one, booting
//!   one on first use when nobody installed anything.
//!
//! # Why one, and why it must not be booted twice
//!
//! A core owns process-global state: the resolved workspace directory, the
//! background services `ServiceSet::embedded` starts (cron, heartbeat, the
//! memory queue), and the credential store. Two of them in one process means
//! two schedulers writing the same memory database and two heartbeats claiming
//! the same account. [`shared`] therefore boots under a [`OnceCell`], so
//! concurrent first callers await one boot rather than racing into several.
//!
//! OpenHuman enforces the same rule from its side now — a second
//! [`Harness`](openhuman_core::Harness) in one process is refused with
//! `HarnessError::AlreadyRunning` rather than quietly sharing the first's
//! keyring, event bus and subscribers. This `OnceCell` is still what makes that
//! refusal unreachable in normal operation: it is the thing that ensures only
//! one boot is ever attempted.
//!
//! # Failure is remembered as failure, not retried forever
//!
//! A boot that fails — no workspace, an unreadable state directory — fails for
//! a reason that will still be true a second later. The error is cloned to
//! every caller rather than re-attempted per node, so a graph with twenty
//! OpenHuman nodes reports one problem twenty times instead of trying to start
//! twenty cores.

use std::sync::Arc;

use tokio::sync::OnceCell;

use openhuman_core::Harness;

/// The process's harness, once someone has installed or booted one.
static SHARED: OnceCell<Result<Arc<Harness>, String>> = OnceCell::const_new();

/// Donate an already-booted core as this process's shared one.
///
/// Called by a host that boots its own — the TUI — so everything else in the
/// process, including a workflow node dispatched to `openhuman`, uses that core
/// rather than starting a second.
///
/// Returns whether this call is the one that installed it. `false` means a core
/// was already present, and *this* one is not it: the caller keeps using the
/// core it holds and the process keeps the first. Reported rather than swapped,
/// because a swap would leave the earlier core's background services running
/// with nothing referring to them.
pub fn install(harness: Arc<Harness>) -> bool {
    SHARED.set(Ok(harness)).is_ok()
}

/// This process's core, booting one if nobody has installed any.
///
/// # Errors
///
/// Returns the boot failure as a string, and returns the *same* failure to
/// every later caller — see the module docs on why a failed boot is not
/// retried.
pub async fn shared() -> Result<Arc<Harness>, String> {
    shared_with_hooks(&crate::harness_hooks::HooksConfig::default()).await
}

/// This process's core, booting one with `hooks` if nobody has installed any.
///
/// The hooks are supplied by the dispatcher rather than read from ambient
/// process state so headless workflow runs preserve the same configured hook
/// policy as a TUI-hosted core.
pub async fn shared_with_hooks(
    hooks: &crate::harness_hooks::HooksConfig,
) -> Result<Arc<Harness>, String> {
    SHARED
        .get_or_init(|| async {
            // A host that never configured the core — a workflow run, an MCP
            // subprocess, a headless daemon — still gets workspace isolation.
            // Whoever had the loaded config installs the full settings with
            // [`super::install_settings`]; this falls back to the floor, which
            // is the workspace derived from this process's Medulla home.
            // Without it a lazily booted core would fall back to the
            // developer's real `~/.openhuman` and read its memory, flows and
            // credentials.
            let settings = super::settings_or_floor();
            super::boot_with_hooks(settings, hooks)
                .await
                .map(Arc::new)
                .map_err(|err| format!("could not start the embedded OpenHuman core: {err}"))
        })
        .await
        .clone()
}

/// Whether a core is already available without booting one.
///
/// For a caller that wants to describe the host rather than use it — a
/// capability advert deciding whether to mention OpenHuman should not start a
/// core as a side effect of answering.
pub fn is_installed() -> bool {
    SHARED.get().is_some_and(Result::is_ok)
}
