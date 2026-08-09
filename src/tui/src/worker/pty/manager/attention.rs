//! Keeping each session's "does this harness want me" flag current.
//!
//! A timer samples each live terminal at rest. Attention state lives on the
//! session handle, so classification never takes the manager registry lock and
//! unrelated harnesses remain independent.

use std::sync::{Arc, Weak};

use super::super::attention::{self, AttentionKind, HarnessAttention};
use super::super::handle::SessionHandle;
use super::super::sync::lock;
use super::session::consumed_bell_count;
use super::PtyManager;

/// How often a session's screen is reclassified.
const ATTENTION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// Minimum time between classifications, slightly below the timer period.
const ATTENTION_INTERVAL_MS: i64 = (ATTENTION_INTERVAL.as_millis() as i64) * 3 / 4;

impl PtyManager {
    /// Sample `handle` on a timer for as long as that session lives.
    ///
    /// Both references are non-owning: a poller cannot keep either the manager
    /// or a reaped session alive.
    pub(super) fn spawn_attention_poller(&self, handle: Weak<SessionHandle>) {
        let inner = Arc::downgrade(&self.inner);
        std::thread::spawn(move || loop {
            std::thread::sleep(ATTENTION_INTERVAL);
            let (Some(handle), Some(inner)) = (handle.upgrade(), inner.upgrade()) else {
                return;
            };
            if !handle.is_running() {
                return;
            }
            refresh(&handle, inner.hook_log.get(), (inner.now)());
        });
    }

    /// Drop `id`'s cue because it has been answered.
    pub fn acknowledge(&self, id: &str) -> bool {
        self.handle(id)
            .is_some_and(|session| session.acknowledge_attention())
    }

    /// What `id` is waiting on the operator for, if anything.
    pub fn attention(&self, id: &str) -> Option<HarnessAttention> {
        self.handle(id)
            .and_then(|session| lock(&session.attention).cue.clone())
    }

    /// How many sessions are waiting on the operator.
    pub fn waiting_count(&self) -> usize {
        self.waiting_sessions().len()
    }

    /// All session IDs that are waiting on the operator.
    ///
    /// Answers from [`row_cue`](super::super::attention::row_cue) rather than
    /// from the screen classification alone, which is what keeps the tab badge
    /// and the rail rows in agreement. They used to disagree by construction:
    /// the rail flags a harness that died or finished, and this counted only the
    /// ones with something on screen, so a badge could read zero above a rail
    /// with two rows blinking on it.
    ///
    /// Only *blocking* cues count — see [`AttentionKind::blocks`]. A finished
    /// session is drawn on its row and left out of the number.
    pub fn waiting_sessions(&self) -> std::collections::HashSet<String> {
        let now = (self.inner.now)();
        self.handles()
            .into_iter()
            .filter(|session| {
                attention::row_cue(&session.row(), now).is_some_and(|cue| cue.kind.blocks())
            })
            .map(|session| session.id().to_string())
            .collect()
    }
}

/// Reclassify one live screen unless it was sampled too recently.
fn refresh(session: &SessionHandle, now: i64) {
    let (seen_bells, generation, pending_completion_bells) = {
        let mut state = lock(&session.attention);
        if now.saturating_sub(state.checked_at) < ATTENTION_INTERVAL_MS {
            return;
        }
        if state
            .completion_deadline
            .is_some_and(|deadline| deadline <= std::time::Instant::now())
        {
            // Operator-entered turns bypass `try_claim`, so the poller also
            // closes expired debt. Bump the revision before sampling so any
            // older in-flight classifier cannot apply the retired count.
            state.pending_completion_bells = 0;
            state.completion_deadline = None;
            state.generation = state.generation.wrapping_add(1);
        }
        state.checked_at = now;
        (
            state.seen_bells,
            state.generation,
            state.pending_completion_bells,
        )
    };

    let (contents, bells) = session.attention_sample();
    let unseen_bells = bells.saturating_sub(seen_bells);
    // Settlement suppresses the exact number of pending completion chimes, not
    // the whole sample. The child can emit delayed chimes and the next turn's
    // request before this 200 ms poll runs; any additional bell remains eligible.
    let eligible_bells = unseen_bells.saturating_sub(pending_completion_bells);
    let working = attention::is_working(&contents);
    let rang = eligible_bells > 0 && !working;
    let cue = attention::detect(session.provider(), &contents)
        .or_else(|| rang.then(|| attention::bell_cue(session.provider())));

    let mut state = lock(&session.attention);
    // Release or acknowledgement raced this sample; it owns the newer truth.
    if state.generation != generation {
        return;
    }
    let consumed_pending = unseen_bells.min(pending_completion_bells);
    state.pending_completion_bells -= consumed_pending;
    if state.pending_completion_bells == 0 {
        state.completion_deadline = None;
    }
    state.seen_bells = consumed_bell_count(state.seen_bells, bells);
    state.working = working;
    state.cue = match (cue, state.cue.take()) {
        (None, held) => held.filter(|held| held.kind == AttentionKind::Bell),
        (Some((kind, what)), None) => Some(HarnessAttention::new(kind, what, now)),
        (Some((kind, what)), Some(held)) => {
            let fresh = HarnessAttention::new(kind, what, now);
            if fresh.supersedes(&held) || fresh.what != held.what {
                Some(fresh)
            } else {
                Some(held)
            }
        }
    };
}
