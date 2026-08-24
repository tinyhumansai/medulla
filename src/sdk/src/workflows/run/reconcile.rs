//! Reconciling run records left behind by a process that went away.
//!
//! [`super::RunFinalizer`] keeps the record honest for every exit path that
//! runs destructors — a cancel, an error, a panic, an orderly shutdown. What it
//! cannot survive is the process not getting to run any code at all: a
//! `SIGKILL`, an OOM kill, a power loss, a laptop lid. Those leave a record
//! saying `running` with nobody running it, and until now nothing ever revisited
//! one. A host that had been killed a few times accumulated dozens of them, and
//! every listing reported them as live work.
//!
//! The fix has two halves, and both are needed:
//!
//! **Ownership.** A record now names the process executing it
//! ([`RunExecutor`]). Without that there is no way to tell "another medulla is
//! working on this right now" from "this is a tombstone", and a sweep that
//! cannot tell the difference is a sweep that eventually kills live work.
//!
//! **Liveness.** [`is_alive`] answers whether that process is still there. It is
//! deliberately conservative: every uncertain case — a record from another host,
//! a platform that will not report a start time, a process table that cannot be
//! read — resolves to "leave it alone". A stale row that survives one sweep is a
//! cosmetic problem; a live run reconciled out from under its executor is a lost
//! run.

use std::sync::Arc;

use crate::workflows::{RunExecutor, RunRecord, RunStatus, WorkflowError, WorkflowStore};

/// What a sweep did to one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// The run that was reconciled.
    pub run_id: String,
    /// The workflow it belonged to.
    pub workflow_id: String,
    /// The status it was given.
    pub status: RunStatus,
}

/// This process's own executor identity.
///
/// Cached: the pid and start time cannot change within a process, and reading
/// the process table is not free.
pub fn current_executor() -> &'static RunExecutor {
    static CURRENT: std::sync::OnceLock<RunExecutor> = std::sync::OnceLock::new();
    CURRENT.get_or_init(|| RunExecutor {
        host: hostname(),
        pid: std::process::id(),
        started_at_secs: process_started_at(std::process::id()),
    })
}

/// This host's name, or `"unknown"` when it cannot be read.
///
/// Only ever compared against another record's copy of the same value, so an
/// unreadable name degrades into "every record looks like it came from
/// somewhere else", which the liveness check treats as not-ours and leaves
/// alone. That is the safe direction.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| sysinfo::System::host_name().filter(|name: &String| !name.trim().is_empty()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// When process `pid` started, in seconds since the epoch.
///
/// `None` when the platform or the permissions will not say, which callers must
/// treat as "cannot rule out pid reuse".
fn process_started_at(pid: u32) -> Option<u64> {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    system.process(pid).map(|process| process.start_time())
}

/// Whether the process named by `executor` is still running.
///
/// Answers `true` for every case it cannot settle, because the caller uses this
/// to decide whether to overwrite a run record and the cost of the two mistakes
/// is not symmetric:
///
/// - A record from another host. This host's process table says nothing about
///   another machine's pids, so comparing them would be a coin flip.
/// - A process that exists but whose start time cannot be read. Without the
///   start time a pid match might be pid reuse, but it might equally be the
///   real executor.
///
/// A start time that is present on both sides and disagrees *is* conclusive:
/// the pid has been recycled, and the process that wrote the record is gone.
pub fn is_alive(executor: &RunExecutor) -> bool {
    let current = current_executor();
    if executor.host != current.host {
        return true;
    }
    if executor.pid == current.pid {
        // A record naming this process's own pid. Ordinarily that means the
        // caller's registry check (which runs first) already ruled the record
        // in or out, and this branch never runs. But a reboot or ordinary pid
        // reuse can hand a *new* process the same pid an old, dead one used,
        // in which case the record's start time predates this process's own —
        // so compare them exactly as the foreign-pid branch below does, rather
        // than trusting a bare pid match.
        return match (executor.started_at_secs, current.started_at_secs) {
            (Some(recorded), Some(actual)) => recorded == actual,
            // One side's start time is unreadable. Cannot disambiguate.
            _ => true,
        };
    }
    let Some(observed) = process_started_at(executor.pid) else {
        return false;
    };
    match executor.started_at_secs {
        // The pid is live and started when the record says it did.
        Some(recorded) => recorded == observed,
        // A live pid we cannot disambiguate. Leave it alone.
        None => true,
    }
}

/// Whether this record is a tombstone: unsettled, but with nobody running it.
///
/// Checks the in-process registry before the process table, so a run this
/// process is executing is never mistaken for an orphan — including during the
/// window where the record has been written but the executor stamp has not.
fn is_orphaned(record: &RunRecord) -> bool {
    if record.status.is_settled() {
        return false;
    }
    if record.status == RunStatus::PendingApproval {
        // Deliberately parked, not abandoned: the CLI process that reached the
        // gate exits normally after writing this status, so the record having
        // no live executor is expected, not evidence of a crash. Reconciling
        // it to `Interrupted` would make the next `resume` reject it as no
        // longer awaiting approval, destroying a run a human simply has not
        // gotten to yet.
        return false;
    }
    if super::is_running(&record.id) {
        return false;
    }
    match &record.executor {
        Some(executor) => !is_alive(executor),
        // Written by a build from before executors were recorded, by a caller
        // that never stamped one, or by a process killed between admitting the
        // run and stamping it. In every case nothing in this process owns it and
        // nothing on the record claims otherwise.
        None => true,
    }
}

/// Settle every run record in `store` that no live process is executing.
///
/// A run whose cancellation was requested settles as [`RunStatus::Cancelled`] —
/// someone asked for that outcome and it is the honest one to record — and
/// everything else as [`RunStatus::Interrupted`], the same status
/// [`super::RunFinalizer`] writes for a run whose process went away.
///
/// Returns what it changed, so a caller can log or report it. Best effort per
/// record: a record that cannot be rewritten is logged and skipped rather than
/// failing the sweep, because one unwritable file should not leave every other
/// tombstone in place.
pub fn reconcile_orphans(store: &Arc<dyn WorkflowStore>) -> Result<Vec<Reconciled>, WorkflowError> {
    let mut reconciled = Vec::new();
    for snapshot in store.unsettled_runs()? {
        if !is_orphaned(&snapshot) {
            continue;
        }
        // Re-read before writing. `unsettled_runs` handed back a snapshot, and
        // between that read and the liveness check the owner can have written a
        // *successful* terminal record and exited — which is precisely why the
        // pid then looks dead. Writing the snapshot back would replace a
        // finished run, and its steps and evidence, with `Interrupted`.
        //
        // The store offers no compare-and-set for run records, so this narrows
        // the window rather than closing it: re-read, re-check, then write.
        // What remains is the microseconds between this read and the write
        // below, against the seconds the liveness check itself takes.
        let mut record = match store.get_run(&snapshot.id) {
            Ok(Some(current)) => current,
            // Vanished under us. Nothing to reconcile.
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(run = %snapshot.id, "could not re-read run before reconciling: {err}");
                continue;
            }
        };
        if record.status.is_settled() {
            // It finished on its own between the snapshot and now. Its own
            // outcome is the true one.
            continue;
        }
        if record.executor != snapshot.executor {
            // A different process picked it up — a resume, most likely — after
            // the snapshot. The liveness verdict was about the old owner and
            // says nothing about this one.
            continue;
        }
        record.status = if record.cancel_requested {
            RunStatus::Cancelled
        } else {
            RunStatus::Interrupted
        };
        record.finished_at = Some(crate::clock::now_millis() as u64);
        if record.error.is_none() && record.status == RunStatus::Interrupted {
            record.error = Some(
                "the process executing this run exited without recording an outcome".to_string(),
            );
        }
        if let Err(err) = store.record_run(&record) {
            tracing::warn!(run = %record.id, "could not reconcile orphaned run: {err}");
            continue;
        }
        reconciled.push(Reconciled {
            run_id: record.id.clone(),
            workflow_id: record.workflow_id.clone(),
            status: record.status,
        });
    }
    Ok(reconciled)
}

/// How long a scope stays swept before [`reconcile_once`] will sweep it again.
///
/// A throttle, not a schedule: nothing wakes up to sweep, so a scope is only
/// re-swept when something discovers its store again. In a short-lived CLI that
/// means once and never; in a long-lived TUI or MCP server it means about this
/// often, as commands come in.
const SWEEP_INTERVAL_MS: u64 = 60_000;

/// Run [`reconcile_orphans`] for the workspace scope named by `scope`, at most
/// once per [`SWEEP_INTERVAL_MS`].
///
/// The sweep hangs off store discovery, which happens many times in one process
/// — the TUI rediscovers per command. Sweeping on every one of those would
/// re-read the runs directory on every keystroke-driven command.
///
/// But a once-per-process guard is wrong in the other direction, and wrong in a
/// way that quietly disables this feature exactly where it matters most. A TUI
/// or MCP server that starts, sweeps, and then runs for hours would never sweep
/// again — so a peer CLI that was alive during that first sweep and is killed
/// ten minutes later leaves a record stuck at `Running` until the long-lived
/// process itself restarts. The orphans this exists to clear are mostly created
/// *after* startup, not before it.
///
/// So: throttle rather than latch. A short-lived process still sweeps once and
/// pays nothing more; a long-lived one keeps catching up on peers that die
/// during its lifetime.
///
/// Keyed by `scope` rather than a single global timestamp: a process that
/// discovers stores for more than one workspace — a different `cwd`, a
/// different `MEDULLA_HOME` — must sweep each of them, not just whichever it
/// saw first. The caller derives `scope` from whatever determines the store's
/// on-disk location, so two discoveries of the *same* workspace share a key
/// even though each call builds a fresh store object.
pub fn reconcile_once(store: &Arc<dyn WorkflowStore>, scope: &str) {
    static SWEPT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    let swept = SWEPT.get_or_init(Default::default);
    let now = crate::clock::now_millis() as u64;
    {
        let mut swept = swept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Stamped before the sweep runs, not after: two threads discovering the
        // same scope at once must not both sweep, and the loser should skip
        // rather than wait on the winner.
        if let Some(last) = swept.get(scope)
            && now.saturating_sub(*last) < SWEEP_INTERVAL_MS
        {
            return;
        }
        swept.insert(scope.to_string(), now);
    }
    match reconcile_orphans(store) {
        Ok(reconciled) if !reconciled.is_empty() => {
            tracing::info!(
                count = reconciled.len(),
                "settled run records left behind by processes that went away"
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!("could not sweep orphaned run records: {err}"),
    }
}
