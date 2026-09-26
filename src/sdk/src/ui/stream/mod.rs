//! Pure derivations over a [`RuntimeSnapshot`](crate::runtime::RuntimeSnapshot)'s
//! event and thread streams.
//!
//! These functions fold the flat event log (and the thread tree) into the small
//! numeric summaries a front end renders — token usage, in-flight call counts,
//! the most recently observed model per tier, and thread nesting depth. They are
//! stateless and side-effect free, so any front end (TUI, CLI, tests) can reuse
//! them.

mod types;

#[cfg(test)]
mod tests;

pub use types::{TierUsage, UsageFold};

use crate::ui::events::{EventEnvelope, TuiEvent};

/// Fold the event log into per-tier and per-task token totals for the Usage tab.
///
/// `InferenceStart` increments a tier's call count; `InferenceEnd` adds its
/// reported input/output tokens; `TaskComplete` accumulates the delegated
/// sub-agent totals and appends a per-task row. Events without usage are ignored.
pub fn usage_fold(events: &[EventEnvelope]) -> UsageFold {
    let mut fold = UsageFold::default();
    for env in events {
        match &env.event {
            TuiEvent::InferenceStart { tier, .. } => {
                fold.tier_mut(tier).calls += 1;
            }
            TuiEvent::InferenceEnd {
                tier,
                usage: Some(u),
                ..
            } => {
                let t = fold.tier_mut(tier);
                t.input_tokens += u.input_tokens;
                t.output_tokens += u.output_tokens;
            }
            TuiEvent::TaskComplete { digest } => {
                if let Some(u) = &digest.usage {
                    fold.subagent.input_tokens += u.input_tokens;
                    fold.subagent.output_tokens += u.output_tokens;
                    fold.subagent.calls += 1;
                    fold.tasks
                        .push((digest.task_id.clone(), u.input_tokens, u.output_tokens));
                }
            }
            _ => {}
        }
    }
    fold
}

/// Count inference calls currently in flight by folding start/end deltas over the
/// event log, clamped at zero so a stray `InferenceEnd` never drives it negative.
pub fn running_calls(events: &[EventEnvelope]) -> i64 {
    let mut n = 0i64;
    for e in events {
        match e.event {
            TuiEvent::InferenceStart { .. } => n += 1,
            TuiEvent::InferenceEnd { .. } => n = (n - 1).max(0),
            _ => {}
        }
    }
    n
}

/// The most recent model id observed on the stream for `tier`, if any.
///
/// Scans the log newest-first and returns the `model` of the first
/// `InferenceStart`/`InferenceEnd` matching the tier that carries one.
pub fn observed_model<'a>(events: &'a [EventEnvelope], tier: &str) -> Option<&'a str> {
    events.iter().rev().find_map(|e| match &e.event {
        TuiEvent::InferenceStart { tier: t, model, .. }
        | TuiEvent::InferenceEnd { tier: t, model, .. }
            if t == tier =>
        {
            model.as_deref()
        }
        _ => None,
    })
}
