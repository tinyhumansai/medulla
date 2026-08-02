//! Pure translation from core wire types into the render snapshot.
//!
//! Kept as free functions with no core and no I/O, deliberately. The fold is
//! where a migration like this actually goes wrong: not with a crash, but with
//! lanes that quietly stop counting because a field moved. Pure functions can
//! be tested exhaustively against hand-written inputs, which is the only way
//! that class of bug gets caught.

use openhuman_core::embed::{EventEnvelope as CoreEnvelope, RosterWorker, SessionSummary};

use crate::runtime::types::{AgentDescriptor, StreamState, ThreadSummary};
use crate::ui::events::{EventEnvelope, TuiEvent};

/// Fold the core's worker roster into render descriptors.
pub fn roster(workers: Vec<RosterWorker>) -> Vec<AgentDescriptor> {
    workers
        .into_iter()
        .map(|w| AgentDescriptor {
            id: w.registry_id,
            name: w.label,
            description: w.description,
            availability: w.availability,
            // The roster carries no placement or provenance, and the render
            // layer reads absent as "not declared". Defaulting the rest keeps
            // this fold honest about what the core actually told us.
            ..AgentDescriptor::default()
        })
        .collect()
}

/// Fold the core's session list into thread summaries.
///
/// The counters (`turns`, `running_tasks`, `attention`) stay zero: the session
/// list carries no per-thread activity, and inventing a value would render as
/// real data. They fill in when the event stream is wired.
pub fn threads(sessions: Vec<SessionSummary>) -> Vec<ThreadSummary> {
    sessions
        .into_iter()
        .map(|s| ThreadSummary {
            id: s.session_id,
            name: s.title.unwrap_or_default(),
            running: false,
            turns: 0,
            running_tasks: 0,
            attention: 0,
        })
        .collect()
}

/// Translate the core's wire envelopes into render envelopes.
///
/// The two `EventEnvelope` types are genuinely different: the core carries a
/// raw `serde_json::Value` payload plus session/cycle routing, while the render
/// layer wants a decoded [`TuiEvent`] and nothing else. `TuiEvent` accepts any
/// `{kind, ...}` object and keeps unrecognized kinds as
/// [`TuiEvent::Unknown`], so a newer backend never drops rows on an older host.
///
/// # Envelopes without a `seq` are dropped, deliberately
///
/// The render layer keys ordering and de-duplication off `seq`. The core's is
/// `Option<u64>`, and mapping `None` to `0` would make every such event look
/// like the oldest in the stream — it would sort to the top and defeat the
/// replay cursor, so a reconnect would re-show it forever. Dropping is the
/// lesser failure and it is logged; a well-behaved backend always sends one on
/// a cursor replay.
pub fn events(envelopes: Vec<CoreEnvelope>) -> Vec<EventEnvelope> {
    let mut out = Vec::with_capacity(envelopes.len());
    for env in envelopes {
        let Some(seq) = env.seq else {
            tracing::debug!(
                "[openhuman_runtime] dropping event with no seq (session={} cycle={:?})",
                env.session_id,
                env.cycle_id
            );
            continue;
        };
        // Infallible in practice: TuiEvent's deserializer falls back to
        // `Unknown` for any object it does not recognize. A failure here means
        // the payload was not an object at all.
        let event: TuiEvent = match serde_json::from_value(env.event) {
            Ok(event) => event,
            Err(err) => {
                tracing::debug!("[openhuman_runtime] undecodable event at seq {seq}: {err}");
                continue;
            }
        };
        out.push(EventEnvelope {
            seq,
            at: env.at as i64,
            event,
        });
    }
    out
}

/// Whether a batch says the turn is running, or nothing at all.
///
/// Cycle boundaries are the only settle signal the event log carries, so the
/// highest-`seq` boundary in the batch decides: `cycle_start` means a turn is
/// producing, `cycle_end` means it finished. A batch with no boundary is
/// mid-turn output and answers `None` — "leave the flag where it was" — rather
/// than `true`. Answering `true` there is what keeps a spinner turning forever
/// after the terminal event, because any straggler behind a `cycle_end`
/// re-asserts a turn that is already over.
pub fn running_after(events: &[EventEnvelope]) -> Option<bool> {
    events
        .iter()
        .filter(|e| {
            matches!(
                e.event,
                TuiEvent::CycleStart { .. } | TuiEvent::CycleEnd { .. }
            )
        })
        .max_by_key(|e| e.seq)
        .map(|e| matches!(e.event, TuiEvent::CycleStart { .. }))
}

/// The highest `seq` in a batch, for advancing the replay cursor.
///
/// `None` for an empty batch, which the caller reads as "cursor unchanged"
/// rather than "reset to the start".
pub fn max_seq(events: &[EventEnvelope]) -> Option<u64> {
    events.iter().map(|e| e.seq).max()
}

/// Map consecutive poll failures onto the header's stream indicator.
///
/// Pure and free-standing so the thresholds are testable without a core; the
/// runtime holds only the counter.
pub fn stream_state(failures: usize, stalled_after: usize) -> StreamState {
    match failures {
        0 => StreamState::Live,
        n if n <= stalled_after => StreamState::Resyncing,
        _ => StreamState::Stalled,
    }
}
