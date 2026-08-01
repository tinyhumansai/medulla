//! Who may use a session next — control handoff and turn claiming.

use std::sync::atomic::Ordering;

use super::super::sync::lock;
use super::super::types::HarnessControl;
use super::super::AttentionKind;
use super::SessionHandle;

impl SessionHandle {
    /// Who holds this session right now.
    pub fn control(&self) -> HarnessControl {
        if self.operator_held.load(Ordering::Acquire) {
            HarnessControl::User
        } else {
            HarnessControl::Orchestrator
        }
    }

    /// Hand the session to `control`.
    ///
    /// Deliberately leaves `busy` alone. The two flags answer different
    /// questions — `busy` is "is a turn running in it", control is "who is
    /// allowed to start one" — and a session taken over mid-turn is still
    /// running that turn. Clearing `busy` on handback would advertise a harness
    /// as free while it was still finishing someone else's work.
    pub(in super::super) fn set_control(&self, control: HarnessControl) {
        self.operator_held
            .store(control == HarnessControl::User, Ordering::Release);
    }

    /// Whether this session may serve `label`'s next turn.
    ///
    /// Its own label, or the synthetic `you:<harness>` one of a handed-back
    /// operator session that has not served a real conversation yet — see
    /// [`adopt_label`](Self::adopt_label).
    pub(in super::super) fn serves_label(&self, label: &str) -> bool {
        let cold = lock(&self.cold);
        cold.label == label || (self.meta.user_spawned && cold.label.starts_with("you:"))
    }

    /// Adopt `label` if this is a handed-back operator session still carrying
    /// its synthetic one.
    ///
    /// Called by the winner of [`try_claim`](Self::try_claim), so the rename
    /// happens once and under the same claim that made it exclusive. After it,
    /// reuse obeys the same exact-label rule as every task-spawned session —
    /// adoption must not turn one harness into a cross-conversation pool.
    pub(in super::super) fn adopt_label(&self, label: &str) {
        let mut cold = lock(&self.cold);
        if self.meta.user_spawned && cold.label.starts_with("you:") {
            cold.label = label.to_string();
        }
    }

    /// Take this session for a turn, if it is free, alive, and the
    /// orchestrator's to take.
    ///
    /// A single compare-exchange, which is what makes the claim atomic without
    /// the registry lock the old find-and-claim needed: two tasks racing for the
    /// same idle session cannot both win, because only one CAS can succeed.
    ///
    /// A session the operator holds is never claimed, however idle it looks.
    /// That is the whole of the unmanaged-harness feature and the whole of
    /// takeover: without it, attaching to a pane and typing does not stop the
    /// orchestrator pasting a task prompt into the same composer — the exact
    /// two-writers collision `busy` exists to prevent, reachable by an operator
    /// simply focusing a harness.
    /// Control is checked again *after* the claim lands, and the claim given
    /// back if it changed: an operator who takes a harness in the window between
    /// the two must win it, because the alternative is a prompt pasted into
    /// their composer a moment after they started typing.
    pub(in super::super) fn try_claim(&self) -> bool {
        if !self.is_running() || !self.control().is_orchestrator() {
            return false;
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if self.control().is_orchestrator() {
            return true;
        }
        self.release();
        false
    }

    /// Mark the session free for the next turn.
    pub(in super::super) fn release(&self) {
        self.busy.store(false, Ordering::Release);
    }

    /// Mark a submitted turn complete and consume its completion chime.
    pub(in super::super) fn settle_turn(&self) {
        let bells = self.bell_count();
        let mut attention = lock(&self.attention);
        let completion_bell_already_accounted_for = bells > attention.seen_bells
            || attention
                .cue
                .as_ref()
                .is_some_and(|cue| cue.kind == AttentionKind::Bell);
        attention.seen_bells = attention.seen_bells.max(bells);
        attention.generation = attention.generation.wrapping_add(1);
        // A bell already represented by the current cue is just as consumed as
        // an unclassified bell past the watermark. In either case, arming
        // suppression here would discard the reused turn's first real request.
        attention.suppress_next_bell = !completion_bell_already_accounted_for;
        attention.cue = attention
            .cue
            .take()
            .filter(|cue| cue.kind != AttentionKind::Bell);
        self.release();
    }

    /// Clear the current cue because a person or injected turn is handling it.
    pub(in super::super) fn acknowledge_attention(&self) -> bool {
        let bells = self.bell_count();
        let mut attention = lock(&self.attention);
        let consumed_suppressed = attention.suppress_next_bell && bells > attention.seen_bells;
        attention.seen_bells = attention.seen_bells.max(bells);
        if consumed_suppressed {
            attention.suppress_next_bell = false;
        }
        attention.generation = attention.generation.wrapping_add(1);
        attention.cue.take().is_some()
    }
}
