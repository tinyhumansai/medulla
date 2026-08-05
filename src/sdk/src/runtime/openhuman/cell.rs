//! The folded snapshot plus its change-notification channel.
//!
//! Split out from the runtime so the render contract — snapshot in, ping out —
//! can be exercised without booting a core. The core uses process globals (a
//! `OnceLock` context, a singleton event bus) and cannot be stood up per test,
//! so anything that genuinely needs one belongs in an integration test. Keeping
//! this piece core-free is what makes the contract unit-testable at all.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use crate::runtime::types::{AgentDescriptor, RuntimeSnapshot, ThreadSummary};
use crate::ui::events::{EventEnvelope, TuiEvent};

/// A turn shown to the operator before the backend has confirmed it.
///
/// Held apart from the snapshot so reconciliation can find the provisional row
/// again without the render layer ever having to know a row was provisional.
struct PendingEcho {
    /// The sentinel `seq` the provisional envelope was filed under.
    seq: u64,
    /// The turn's text, matched against the backend's own copy.
    body: String,
}

/// Bookkeeping for turns echoed locally and not yet confirmed.
#[derive(Default)]
struct EchoState {
    /// Provisional turns, oldest first — the order the backend confirms them in.
    pending: Vec<PendingEcho>,
    /// When the oldest unconfirmed echo was placed, for the stall watchdog.
    ///
    /// Re-armed only while nothing has come back at all: the watchdog answers
    /// "did the backend say anything about this turn", not "is the cycle slow",
    /// which is [`StreamState`](crate::runtime::types::StreamState)'s question.
    armed_at: Option<Instant>,
}

/// Holds the folded view and notifies readers when it changes.
pub struct SnapshotCell {
    /// A plain mutex, not an async one: every write is a whole-snapshot swap,
    /// so the lock is never held across an await and there is no partial state
    /// for a reader to observe.
    state: Mutex<RuntimeSnapshot>,
    /// Provisional turns awaiting the backend's confirmation.
    ///
    /// **Lock order: `state` before `echoes`, always.** The two are separate
    /// mutexes so a reader calling [`snapshot`](Self::snapshot) never contends
    /// on echo bookkeeping, which makes a consistent acquisition order the only
    /// thing standing between this and a deadlock.
    echoes: Mutex<EchoState>,
    /// Payload-free ping. The contract is "something moved, re-read the
    /// snapshot", which keeps a slow reader from having to replay a backlog and
    /// makes a lagging receiver harmless.
    tx: broadcast::Sender<()>,
}

impl SnapshotCell {
    /// An empty cell.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(16);
        Self {
            state: Mutex::new(RuntimeSnapshot::default()),
            echoes: Mutex::new(EchoState::default()),
            tx,
        }
    }

    /// The current folded view.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Subscribe to change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    /// Replace the roster and thread list, then notify.
    pub fn apply(&self, roster: Vec<AgentDescriptor>, threads: Vec<ThreadSummary>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.roster = roster;
            state.threads = threads;
        }
        // An absent or lagging receiver is not an error — the ping is advisory
        // and the next `snapshot()` reads the same state regardless.
        let _ = self.tx.send(());
    }

    /// Append newly replayed events and notify.
    ///
    /// Appends rather than replaces: events are a growing log, and the caller
    /// only ever fetches what is past its cursor. De-duplication is the
    /// cursor's job, not this method's — re-applying an old batch here would
    /// duplicate rows rather than being idempotent.
    ///
    /// A `running` flag rides along because it is derived from the same fetch:
    /// the caller knows whether the turn settled, and splitting it into a
    /// second call would let the two disagree between locks.
    ///
    /// `None` means "the fetch said nothing about liveness" and leaves the flag
    /// alone. Most batches are mid-turn output that carries no cycle boundary,
    /// and forcing a `bool` there would make every such batch re-assert
    /// `running = true` — which is how a settled turn ends up spinning forever
    /// once a late event arrives behind its `cycle_end`.
    pub fn append_events(&self, events: Vec<EventEnvelope>, running: Option<bool>) {
        if events.is_empty() {
            // Still record liveness — a turn can settle without new events.
            let Some(running) = running else {
                return;
            };
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.running == running {
                return;
            }
            state.running = running;
        } else {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut echoes = self.echoes.lock().unwrap_or_else(|e| e.into_inner());
            // A batch arrived, so the backend is talking about this turn: the
            // stall watchdog has nothing left to answer.
            echoes.armed_at = None;
            for env in &events {
                let TuiEvent::User { body } = &env.event else {
                    continue;
                };
                // The backend's own copy of a turn this host already drew.
                // Retire the provisional row rather than rendering the turn
                // twice; the confirmed envelope carries the real `seq`, and is
                // appended below in the batch's own order.
                //
                // Matched by body against the oldest pending echo. Safe within
                // a thread because the cursor only moves forward, so a turn is
                // never re-delivered — and `switch_thread`, the one place that
                // does replay from the start, drops the pending list with the
                // transcript it belonged to.
                //
                // The retirement itself identifies the row by *identity*, not
                // by `seq`, because the provisional's `seq` is a guess that an
                // earlier batch's real event can already be sitting on.
                let Some(index) = echoes.pending.iter().position(|p| &p.body == body) else {
                    continue;
                };
                let retired = echoes.pending.remove(index);
                remove_provisional(&mut state.events, &retired);
                remove_provisional(&mut state.chat_events, &retired);
            }
            drop(echoes);
            state.events.extend(events.iter().cloned());
            // The chat view wants only conversational rows; everything else is
            // trace. Splitting here keeps the render layer from re-filtering
            // the whole log on every frame.
            state
                .chat_events
                .extend(events.into_iter().filter(is_chat_row));
            if let Some(running) = running {
                state.running = running;
            }
        }
        let _ = self.tx.send(());
    }

    /// Draw the operator's own turn immediately, before the backend has echoed
    /// it, and mark the turn running.
    ///
    /// The alternative — waiting for the turn to come back over the replay —
    /// costs a full poll interval plus two round trips before a submitted line
    /// appears anywhere, which reads as dropped input rather than as latency.
    ///
    /// The provisional envelope is filed one past the highest `seq` on record,
    /// which is both where the transcript wants it and, in the ordinary case,
    /// the `seq` the backend is about to assign. It is retired by
    /// [`append_events`](Self::append_events) when the confirmed copy arrives,
    /// so nothing downstream ever sees a row that is provisional *and* stale.
    ///
    /// Returns the sentinel `seq`, for [`abandon_echo`](Self::abandon_echo).
    pub fn echo_user(&self, body: &str) -> u64 {
        let seq = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let seq = state.events.last().map_or(1, |e| e.seq.saturating_add(1));
            let envelope = EventEnvelope {
                seq,
                at: crate::clock::now_millis(),
                event: TuiEvent::User {
                    body: body.to_string(),
                },
            };
            state.events.push(envelope.clone());
            state.chat_events.push(envelope);
            state.running = true;

            let mut echoes = self.echoes.lock().unwrap_or_else(|e| e.into_inner());
            echoes.pending.push(PendingEcho {
                seq,
                body: body.to_string(),
            });
            echoes.armed_at.get_or_insert_with(Instant::now);
            seq
        };
        let _ = self.tx.send(());
        seq
    }

    /// Give up on a turn the backend never accepted, keeping its row.
    ///
    /// The send failed, so no confirmation is ever coming and leaving the echo
    /// pending would let it swallow the next identical turn. The row itself
    /// stays: the operator wrote it, and a line that vanishes on a failed send
    /// takes the text with it. The spinner settles, because nothing is running.
    pub fn abandon_echo(&self, seq: u64) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut echoes = self.echoes.lock().unwrap_or_else(|e| e.into_inner());
            echoes.pending.retain(|p| p.seq != seq);
            if echoes.pending.is_empty() {
                echoes.armed_at = None;
                state.running = false;
            }
        }
        let _ = self.tx.send(());
    }

    /// Settle the spinner when an accepted turn has produced nothing at all.
    ///
    /// The backend took the message — the POST was accepted — and then said
    /// nothing for `timeout`: no echo, no cycle boundary, no output. Spinning
    /// on indefinitely claims work is in flight that this host has no evidence
    /// of, so the indicator stops while the turn stays in the transcript.
    ///
    /// Disarmed rather than repeating: the next batch to arrive re-establishes
    /// liveness the ordinary way, and a re-armed watchdog would fight it.
    /// Returns whether it fired, so the caller can say so.
    pub fn expire_stalled_echo(&self, timeout: Duration) -> bool {
        let fired = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut echoes = self.echoes.lock().unwrap_or_else(|e| e.into_inner());
            match echoes.armed_at {
                Some(at) if at.elapsed() >= timeout => {
                    echoes.armed_at = None;
                    state.running = false;
                    true
                }
                _ => false,
            }
        };
        if fired {
            let _ = self.tx.send(());
        }
        fired
    }

    /// The thread currently selected, or empty when none has been.
    pub fn active_thread_id(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_thread_id
            .clone()
    }

    /// Record the operator's active thread.
    pub fn set_active_thread(&self, id: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active_thread_id = id;
    }

    /// Point the snapshot at a different thread and drop the previous one's log.
    ///
    /// The transcript is per-thread, so keeping the old rows would render two
    /// conversations as one — and the incoming thread replays from its own
    /// start, which would interleave them by arrival rather than by thread.
    /// `running` resets too: the previous thread's liveness says nothing about
    /// this one, and the first batch re-establishes it.
    pub fn switch_thread(&self, id: String) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.active_thread_id = id;
            state.events.clear();
            state.chat_events.clear();
            state.running = false;
            // Provisional rows belong to the transcript being dropped. Kept,
            // they would match the incoming thread's replay by body alone and
            // retire rows this host never drew.
            let mut echoes = self.echoes.lock().unwrap_or_else(|e| e.into_inner());
            echoes.pending.clear();
            echoes.armed_at = None;
        }
        let _ = self.tx.send(());
    }
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Drop the one provisional row a confirmation retires, leaving any other row
/// that happens to share its `seq`.
///
/// [`echo_user`](SnapshotCell::echo_user) files the provisional one past the
/// highest `seq` on record — a guess at what the backend will assign, not a
/// reservation. A batch that carries a non-user event at that `seq` and no
/// matching turn lands the real event alongside the provisional, so removing
/// everything at the `seq` would take the real event with it. Losing a
/// `cycle_start` that way is not cosmetic: [`fold::running_after`] reads
/// liveness off cycle boundaries and the agents view counts turns from them.
///
/// So the row is identified by its body as well as its `seq` — only a `User`
/// row echoing exactly what was retired can be the provisional — and removed
/// by position.
///
/// [`fold::running_after`]: crate::runtime::openhuman::fold::running_after
fn remove_provisional(rows: &mut Vec<EventEnvelope>, retired: &PendingEcho) {
    let provisional = rows.iter().position(|e| {
        e.seq == retired.seq && matches!(&e.event, TuiEvent::User { body } if *body == retired.body)
    });
    if let Some(index) = provisional {
        rows.remove(index);
    }
}

/// Whether an event belongs in the chat transcript rather than the trace.
///
/// Deliberately a small allow-list rather than a deny-list: a new event kind
/// added upstream should default to *trace*, where an unexpected row is noise,
/// not to the transcript, where it would read as something the user or the
/// assistant said.
fn is_chat_row(env: &EventEnvelope) -> bool {
    matches!(
        env.event,
        TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::AssistantDelta { .. }
    )
}
