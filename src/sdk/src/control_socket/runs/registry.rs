//! The table reported runs accumulate in, and the rules that bound it.
//!
//! Two bounds and one lifetime. The bounds are retention: a session keeps its
//! most recent runs and a run keeps its most recent frames, so a harness that
//! starts run after run — or a step that streams for an hour — costs a fixed
//! amount of memory rather than a growing one.
//!
//! The lifetime is *retirement*, and it is the reason this is not simply a map.
//! A default `workflow_run` is asynchronous: the tool answers as soon as the run
//! is started, and the run itself keeps executing in the MCP subprocess, which
//! deliberately outlives its parent harness's stdin (see
//! [`crate::mcp`]'s server loop, which waits on run liveness before returning).
//! So the harness routinely exits while the run it started is still going. If
//! the harness's exit tore this table down, the operator would watch the most
//! consequential thing the session did vanish from the rail mid-flight and
//! never learn how it ended. Instead the session is *retired*: its rows stay,
//! and the reporting side keeps a grant good for nothing but `run.report` until
//! the last run it left behind settles.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::types::{HarnessRun, HarnessRunFrame, RunReport};

/// The most runs retained per session.
///
/// A session that starts run after run keeps its recent ones; the rest is the
/// workflow's own durable history, which the Workflows page reads from disk.
pub(crate) const MAX_RUNS_PER_SESSION: usize = 8;

/// The most progress frames retained per run.
///
/// Enough to watch a step work; the run's own record is where the durable
/// account of it lives.
pub(crate) const MAX_FRAMES_PER_RUN: usize = 200;

/// Everything the registry holds, behind one lock.
#[derive(Default)]
struct Table {
    /// Runs per grant session, oldest first.
    sessions: HashMap<String, Vec<HarnessRun>>,
    /// Sessions whose harness has exited with a run still unsettled.
    ///
    /// Membership is what keeps a reporting-only grant alive: it is added by
    /// [`HarnessRunRegistry::retire`] and removed when the last unsettled run
    /// reports a terminal status, which is the moment the grant can go.
    retiring: HashSet<String>,
}

/// The runs every granted session has reported, keyed by grant session.
///
/// Cheap to clone; every clone shares one table.
#[derive(Clone, Default)]
pub struct HarnessRunRegistry {
    inner: Arc<Mutex<Table>>,
}

impl HarnessRunRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `report` against `session`, creating the run on first sight.
    ///
    /// A repeat report updates the run in place, so a run that emits a hundred
    /// progress lines stays one row rather than a hundred.
    ///
    /// Returns `true` when this report was the one that finished retiring the
    /// session: its harness had already exited, and this settled the last run
    /// it left executing. Nothing can be reported under that session again, so
    /// the caller gives back the reporting-only grant it was holding open. Any
    /// other report — including every report from a session whose harness is
    /// still alive — returns `false`.
    pub fn report(&self, session: &str, report: RunReport) -> bool {
        let now = crate::clock::now_millis();
        let Ok(mut table) = self.inner.lock() else {
            return false;
        };
        let entry = table.sessions.entry(session.to_string()).or_default();
        if let Some(existing) = entry.iter_mut().find(|run| run.run_id == report.run_id) {
            existing.status = report.status;
            existing.updated_at = now;
            // A report with nothing to say still moves the clock; it must not
            // blank out the last thing that did.
            if let Some(detail) = report.detail {
                push_frame(existing, report.node, detail);
            }
            return finish_retiring_if_settled(&mut table, session);
        }
        let mut run = HarnessRun {
            run_id: report.run_id,
            workflow_id: report.workflow_id,
            status: report.status,
            started_at: now,
            updated_at: now,
            detail: None,
            frames: Vec::new(),
        };
        if let Some(detail) = report.detail {
            push_frame(&mut run, report.node, detail);
        }
        entry.push(run);
        // Oldest settled first: a run still executing is the one the operator
        // is watching, and dropping it to keep a finished one would be exactly
        // backwards.
        while entry.len() > MAX_RUNS_PER_SESSION {
            let victim = entry
                .iter()
                .position(|run| run.status.is_terminal())
                .unwrap_or(0);
            entry.remove(victim);
        }
        finish_retiring_if_settled(&mut table, session)
    }

    /// The run with this id, wherever it was reported from.
    ///
    /// Runs are keyed by session because that is how they are *drawn*; a reader
    /// that already has a run id — the Workflows page, following a jump from
    /// the rail — has no session to look it up under.
    pub fn find(&self, run_id: &str) -> Option<HarnessRun> {
        self.inner
            .lock()
            .ok()?
            .sessions
            .values()
            .flatten()
            .find(|run| run.run_id == run_id)
            .cloned()
    }

    /// Whether `session` has reported any run at all.
    ///
    /// Separate from [`for_session`](Self::for_session) because a reader that
    /// only wants the yes/no would otherwise clone every run to throw them
    /// away. A surface asking whether there is still something to watch wants
    /// [`any_active_for_session`](Self::any_active_for_session) instead — this
    /// one stays `true` for a session whose runs have all settled.
    pub fn any_for_session(&self, session: &str) -> bool {
        self.inner.lock().ok().is_some_and(|table| {
            table
                .sessions
                .get(session)
                .is_some_and(|runs| !runs.is_empty())
        })
    }

    /// Whether `session` has a run that has not settled yet.
    ///
    /// A narrower question than [`any_for_session`](Self::any_for_session), and
    /// the one a surface that prunes finished work asks: a session whose runs
    /// have all settled is one nothing is still waiting on, even though it did
    /// report runs and so still answers `true` there.
    pub fn any_active_for_session(&self, session: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|table| has_unsettled(&table, session))
    }

    /// What `session` has running and recently ran, oldest first.
    pub fn for_session(&self, session: &str) -> Vec<HarnessRun> {
        self.inner
            .lock()
            .ok()
            .and_then(|table| table.sessions.get(session).cloned())
            .unwrap_or_default()
    }

    /// Note that `session`'s harness has exited, and say whether the reporting
    /// side still needs to be able to reach us.
    ///
    /// `true` means the session left at least one run unsettled: the run is
    /// still executing in an MCP subprocess that outlived its parent, so the
    /// caller keeps a reporting-only grant alive for it and this registry keeps
    /// the rows. The [`report`](Self::report) that settles the last of them
    /// returns `true` in turn, which is where that grant is given back.
    ///
    /// `false` means there is nothing left to hear from and the session's grant
    /// can be revoked outright — the ordinary case, since most sessions never
    /// start a workflow run at all.
    ///
    /// The rows are *not* dropped either way. They belong to the PTY row, whose
    /// screen is deliberately kept after its child exits so the operator can
    /// read how the session ended; [`forget`](Self::forget) is called when that
    /// row itself goes.
    pub fn retire(&self, session: &str) -> bool {
        let Ok(mut table) = self.inner.lock() else {
            return false;
        };
        if !has_unsettled(&table, session) {
            table.retiring.remove(session);
            return false;
        }
        table.retiring.insert(session.to_string());
        true
    }

    /// Forget everything `session` reported.
    ///
    /// Called when the operator drops the session's row: the runs were drawn
    /// underneath it, and keeping them past it would be a growing table keyed
    /// by sessions nothing draws.
    pub fn forget(&self, session: &str) {
        if let Ok(mut table) = self.inner.lock() {
            table.sessions.remove(session);
            table.retiring.remove(session);
        }
    }
}

/// Whether `session` has a run that has not settled.
///
/// A run parked on an approval gate counts as unsettled: the gate is answered
/// in the subprocess that is still running it, so there is more to hear.
fn has_unsettled(table: &Table, session: &str) -> bool {
    table
        .sessions
        .get(session)
        .is_some_and(|runs| runs.iter().any(|run| !run.status.is_terminal()))
}

/// Clear `session`'s retiring mark if this report settled its last run, and say
/// whether that just happened.
///
/// Only ever `true` for a session already retiring, so a live harness's reports
/// never look like the end of one.
fn finish_retiring_if_settled(table: &mut Table, session: &str) -> bool {
    if !table.retiring.contains(session) || has_unsettled(table, session) {
        return false;
    }
    table.retiring.remove(session);
    true
}

/// Record one frame on `run`, keeping the newest and dropping past the bound.
fn push_frame(run: &mut HarnessRun, node: Option<String>, text: String) {
    run.detail = Some(text.clone());
    run.frames.push(HarnessRunFrame { node, text });
    if run.frames.len() > MAX_FRAMES_PER_RUN {
        let excess = run.frames.len() - MAX_FRAMES_PER_RUN;
        run.frames.drain(..excess);
    }
}
