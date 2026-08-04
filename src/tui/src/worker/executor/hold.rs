//! Operator holds: queueing behind one, suspending a turn to one, and the
//! hand-back turn that turns one back into a task result.
//!
//! Three moments, one fact: a session an operator holds is **not a dispatch
//! candidate** (spec §4.1). Everything here follows from that.
//!
//! 1. **Before a turn** — nothing reusable, and the checkout already has a
//!    writer in it: the dispatch *queues* behind them
//!    ([`await_checkout_release`](PtySessionExecutor::await_checkout_release)).
//!    It used to be refused outright, which made a person at a keyboard a task
//!    failure the orchestrator had to route around.
//!
//! Note the grain, because it is easy to lose: **holds are on sessions**
//! ([`SessionControl`]), never on directories. What is per-*checkout* is the
//! serialization rule the strategy imposes
//! ([`checkout_writer`](PtySessionExecutor::checkout_writer)) — a separate rule
//! that today happens to be triggered by the same event.
//! 2. **During a turn** — the operator takes the session mid-flight: the turn
//!    *suspends* ([`await_handback`](super::super::PtySessionExecutor::await_handback))
//!    rather than being discarded, keeping everything the harness has produced
//!    so far and holding the task open.
//! 3. **After the hold** — control comes back: a fresh **hand-back turn** runs
//!    in that same session ([`handback_prompt`]), because after minutes of
//!    operator activity the suspended continuation may be stale. The session is
//!    the only context that saw both the agent's partial work and the person's,
//!    so its answer is the authoritative result — and it is emitted as the
//!    pending task's own result, under the same task id.
//!
//! The one thing none of this does is *fail*. A dispatch that meets a person
//! ends in a real result or a real error, never in silence and never in a
//! refusal the orchestrator has to interpret.

use std::time::Duration;

use medulla::daemon::mappers::HarnessSemanticEvent;
use medulla::daemon::providers::{Abort, OnEvent};
use medulla::protocol::{HarnessEvent, HarnessProvider};

use super::super::pty::{SessionControl, SessionRow};
use super::types::PtySessionExecutor;

/// How often a queued or suspended dispatch re-checks who holds the session.
///
/// A human timescale, not a queue-depth one: nothing here is waiting on a
/// machine. Cheap enough (one atomic read per live session, plus a path compare
/// for the queue case) that a task parked behind someone's lunch break costs
/// nothing measurable.
const HOLD_POLL: Duration = Duration::from_millis(250);

/// The queue budget for a caller that stated no idle ceiling of its own.
///
/// Only a floor against an unbounded wait — `[host].taskTimeoutMs` is always set
/// in practice, so this is never what bounds a real dispatch.
const DEFAULT_QUEUE_BUDGET: Duration = Duration::from_secs(600);

/// The instruction a hand-back turn is given, wrapping the task's own.
///
/// Deliberately **not** a resumption of the suspended turn (spec §5.1): after
/// minutes of operator activity that continuation may be stale — the person may
/// have finished the work themselves, advanced it partly, or changed the tree
/// under it. So the turn is fresh, and it is told to *look* before it acts.
///
/// One line, because it is typed into a composer. A line-oriented harness reads
/// a prompt up to the first newline, so a multi-line brief would arrive as a
/// prompt plus stray input.
pub(super) fn handback_prompt(instruction: &str) -> String {
    format!(
        "An operator took control of this session and has been working in it; \
         you now have it back. Review this session's history and the current \
         state of the workspace, then finish this task: {instruction} — if the \
         work is already done, do not redo it: report the final result. \
         Otherwise continue from where things now stand and complete it.",
    )
}

/// The `status` frame text that announces a hold to the requester.
///
/// Built from the shared prefix so the hub recognises it and pauses that
/// dispatch's no-progress watchdog for as long as the hold lasts — a person
/// reading their session is not a crashed worker.
fn held_status(provider: HarnessProvider) -> String {
    format!(
        "{} · the {} turn is suspended, not lost",
        medulla::daemon::SESSION_HELD_STATUS_PREFIX,
        provider.as_str(),
    )
}

/// The `status` frame text that ends a hold, on the same shared-prefix contract.
fn resumed_status(provider: HarnessProvider) -> String {
    format!(
        "{} · the {} session is reviewing what changed",
        medulla::daemon::SESSION_RESUMED_STATUS_PREFIX,
        provider.as_str(),
    )
}

/// A synthetic `status` event, so a control change reaches the peer through the
/// channel every other bit of progress already uses.
///
/// Synthesised rather than folded from a transcript because no harness writes a
/// record for "a human took the keyboard" — it is a fact about the *session*,
/// not about the turn. [`crate::worker::executor`] is the only place that knows
/// it.
fn control_event(detail: String, state: &str) -> HarnessSemanticEvent {
    HarnessSemanticEvent {
        line: 0,
        timestamp_ms: medulla::clock::now_millis(),
        record_type: "medulla:control".to_string(),
        event: HarnessEvent {
            kind: "status".to_string(),
            payload: serde_json::json!({ "state": state, "detail": detail }),
            ..Default::default()
        },
    }
}

/// Tell the peer a person has the session.
pub(super) fn report_held(on_event: &mut Option<OnEvent>, provider: HarnessProvider) {
    if let Some(callback) = on_event.as_mut() {
        callback(&control_event(held_status(provider), "held"));
    }
}

/// Tell the peer the session is back and the hand-back turn is starting.
pub(super) fn report_resumed(on_event: &mut Option<OnEvent>, provider: HarnessProvider) {
    if let Some(callback) = on_event.as_mut() {
        callback(&control_event(resumed_status(provider), "running"));
    }
}

impl PtySessionExecutor {
    /// The session a new one would collide with if it started in `cwd` — the
    /// **serialization** rule, which is about the strategy, not about control.
    ///
    /// Two rules govern where a dispatch may run, and E deliberately keeps them
    /// apart because they only *coincide* today:
    ///
    /// 1. **Candidacy** (session grain, spec §4.1) — a user-owned session is not
    ///    a dispatch candidate. That is decided per session, by
    ///    [`claim_idle`](crate::worker::pty::PtyManager::claim_idle), and says
    ///    nothing whatever about the session's neighbours.
    /// 2. **Serialization** (strategy grain, spec §2.3) — under
    ///    `strategy: checkout` every session of an agent shares one working
    ///    tree, so the tree takes one writer at a time. That is decided per
    ///    *checkout*, and it is why a fresh session cannot simply start beside
    ///    an existing writer.
    ///
    /// Conflating them is what produced the behaviour this replaces: a hold on
    /// one session refused the whole directory, so a person opening a session to
    /// read it failed dispatches that had nothing to do with them. Under
    /// `worktree` (phase G) the two separate visibly — sessions get their own
    /// trees, rule 2 stops applying, and a held session will correctly imply
    /// nothing at all about its siblings, with no control logic to revisit.
    ///
    /// **What this enforces today, and what it does not.** It prevents the one
    /// pair of concurrent writers that nothing else does: the orchestrator
    /// starting a harness in a tree a *person* is working in. It does **not**
    /// serialize two orchestrator sessions in one checkout — main opens a second
    /// session for a second concurrent dispatch (pinned by
    /// `concurrent_tasks_from_one_peer_do_not_share_a_session`), and turning
    /// that into a queue is a behaviour change with its own capacity story:
    /// F3 (per-agent serial queue, semaphore demoted to a backstop) owns it.
    /// The gap is stated rather than silently closed, because closing it here
    /// would look like a control fix and would in fact be a scheduling one.
    pub(super) fn checkout_writer(&self, cwd: &str) -> Option<SessionRow> {
        self.sessions
            .sessions_in(cwd)
            .into_iter()
            .find(|row| row.control == SessionControl::User)
    }

    /// How long a queued dispatch may wait for the checkout before giving up.
    ///
    /// The caller's own idle ceiling (`[host].taskTimeoutMs`), because that is
    /// the requester's stated patience for this task and waiting for a person is
    /// not different in kind from waiting for a harness. `0` means the caller
    /// set no ceiling, which is never observed from `[host]` — its default is
    /// nonzero — so the fallback exists only so an unset budget cannot become an
    /// unbounded wait.
    pub(super) fn queue_budget(&self, timeout_ms: u64) -> Duration {
        if timeout_ms == 0 {
            DEFAULT_QUEUE_BUDGET
        } else {
            Duration::from_millis(timeout_ms)
        }
    }

    /// Wait for the checkout at `cwd` to take a writer, or give up at
    /// `deadline`.
    ///
    /// The queue, and the whole of what replaced the blanket refusal: the same
    /// exclusivity — never two writers in one working tree — bought by waiting
    /// rather than by ending the dispatch.
    ///
    /// Keyed on the directory rather than on one session id because the wait is
    /// on the *tree*: the operator may close the session they are in and open
    /// another, and the next writer is as much of a collision as the last one.
    ///
    /// # Errors
    ///
    /// - The orchestrator aborted while the task was queued — nothing has been
    ///   started, so there is nothing to stop.
    /// - The budget ran out with the person still working. Reported with the
    ///   shared held prefix so the hub settles it as
    ///   [`RunError::Held`](medulla::hub::RunError::Held) — "I did not attempt
    ///   this, come back later" — rather than as a task the harness tried and
    ///   failed. This is the one path that still reaches that refusal, and it
    ///   exists so a dispatch always ends in a real answer: silence for a
    ///   ten-minute lunch break would be worse than a retryable no.
    pub(super) async fn await_checkout_release(
        &self,
        cwd: &str,
        abort: &Abort,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        loop {
            let Some(held) = self.checkout_writer(cwd) else {
                return Ok(());
            };
            if abort.is_aborted() {
                return Err("task aborted while queued behind an operator".to_string());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "{}: an operator is working in {cwd} ({} session {})",
                    medulla::daemon::HARNESS_HELD_PREFIX,
                    held.provider.as_str(),
                    held.id,
                ));
            }
            tokio::time::sleep(HOLD_POLL).await;
        }
    }

    /// Suspend the turn running in `id` until the operator hands it back.
    ///
    /// Unbounded on purpose. A hold is a person's decision and lasts a person's
    /// amount of time; the watchdogs that would otherwise reap the task are
    /// *paused* for its duration rather than merely lengthened — the worker's own
    /// idle ceiling by not accruing here (the caller resets it on resume), and
    /// the requester's no-progress window by the held status frame this
    /// suspension announces. What still ends the wait is an event, not a clock:
    /// the operator hands it back, the orchestrator aborts, or the session dies.
    ///
    /// # Errors
    ///
    /// - Aborted while held. The session is left exactly as it is: it belongs to
    ///   the operator now, and interrupting or closing it would take their work
    ///   away to settle a task they are no longer part of.
    /// - The session ended while the operator had it — there is no longer
    ///   anything to hand back or to run a hand-back turn in.
    pub(super) async fn await_handback(
        &self,
        id: &str,
        provider: HarnessProvider,
        abort: &Abort,
    ) -> Result<(), String> {
        loop {
            if self.sessions.control(id) != Some(SessionControl::User) {
                return Ok(());
            }
            if abort.is_aborted() {
                return Err(format!(
                    "{} task aborted while an operator held the session",
                    provider.as_str(),
                ));
            }
            if !self
                .sessions
                .row(id)
                .is_some_and(|row| row.state.is_running())
            {
                return Err(format!(
                    "{} session ended while an operator held it",
                    provider.as_str(),
                ));
            }
            tokio::time::sleep(HOLD_POLL).await;
        }
    }
}
