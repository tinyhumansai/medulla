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

/// How old the most recent lifecycle report may be while still holding
/// `working` over an idle screen.
///
/// `run_hook_cmd` (`commands/hook.rs`) deliberately swallows a report's
/// delivery failure or timeout, so a session whose `Stop` report never made it
/// would otherwise latch `working` for the rest of its life once the idle
/// screen stopped ending a turn on its own. This does not touch the case the
/// hook priority exists for: a genuine spinner or working marker on screen
/// always wins immediately, composer or no composer, with no grace window
/// involved at all (see [`working_state`]'s first check). The grace window
/// only bounds trust in a report claiming activity while the screen shows
/// *no* live work whatsoever, and it is measured from that report's own
/// [`HookReport::at_ms`](medulla::harness_hooks::HookReport::at_ms) rather than
/// from how long the screen has looked idle — a session that resumes from a
/// long-idle prompt (a new `UserPromptSubmit`, or the next tool report after a
/// permission menu) is fresh evidence the instant it is recorded, however long
/// the screen sat idle before it. Set generously above the ordinary
/// composer-settles-before-Stop-is-filed race (one hook dispatch, on the order
/// of the poll interval) and above a legitimately silent mid-turn stretch (a
/// slow tool result streaming in, compaction) so neither is mistaken for a
/// dropped report; only a report this old, with nothing fresher and no
/// corroborating spinner, is presumed to have lost its `Stop`.
pub(super) const HOOK_STALE_GRACE_MS: i64 = 30_000;

/// How long a session may produce *no* pty output at all before an open-ended
/// report (`hook_report_is_open_ended`: a tool, subagent, or compaction that
/// started and has not returned) is presumed abandoned rather than trusted.
///
/// An open-ended report is deliberately not aged by [`HOOK_STALE_GRACE_MS`] —
/// a slow build can run far longer than that while genuinely working. But
/// "no natural upper bound" is not "no bound at all": if its matching
/// `Post`/`Stop` event is lost the same way a plain `Stop` can be
/// (`run_hook_cmd` swallows delivery failures), the report would otherwise
/// hold `working` forever with nothing to ever reconsider it, which is worse
/// than the bounded case this whole mechanism exists to fix. Total silence —
/// not even a keystroke echoed back — for this long is what an abandoned
/// report looks like; set far above [`HOOK_STALE_GRACE_MS`] because a
/// legitimately silent long-running tool (no verbose output) is far more
/// plausible than a legitimately silent long *reply*, and must not be cut off
/// on the same short clock a boundary report is.
pub(super) const OPEN_ENDED_ABANDON_MS: i64 = 10 * 60_000;

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
fn refresh(
    session: &SessionHandle,
    hook_log: Option<&medulla::harness_hooks::HookEventLog>,
    now: i64,
) {
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
    // One snapshot of the session's most recent lifecycle report, read once and
    // reused for both "is this session active" and "how old is that report":
    // two separate reads a report apart could otherwise pair a fresh event
    // with a stale one's age (or vice versa) if a new report lands on the log
    // in between, misclassifying the sample either as stale when it just
    // renewed or as fresh on an age that never applied to the new event.
    let last_report = hook_log.and_then(|log| {
        log.recent_for(session.grant_session()?, 1)
            .into_iter()
            .next()
    });
    let hook_active = last_report
        .as_ref()
        .map(|report| attention::hook_event_marks_active_turn(report.event));
    // Whether the report opens an operation of unbounded duration rather than
    // marking a boundary after which visible activity is expected imminently
    // — a tool, subagent, or compaction that has *started* and not yet
    // returned (`hook_report_is_open_ended`) has no legitimate upper bound on
    // how long it can run while genuinely still working (a slow build under
    // `Bash` can easily outlast any fixed grace). Kept as its own fact rather
    // than folded into `report_age_ms` as `None`: age and open-endedness are
    // independent properties of the report, and a fresh open-ended report
    // needs both — its youth to prove life at once, and its open-endedness to
    // stay untouched by the fast clock once it is no longer young. Encoding
    // one as the absence of the other made that impossible to say.
    let report_open_ended = last_report
        .as_ref()
        .is_some_and(|report| attention::hook_report_is_open_ended(report.event));
    // How old that report is, measured for every report alike — open-ended or
    // not. `working_state` is what decides which reports the age actually
    // gates; this is just the fact.
    let report_age_ms = last_report
        .as_ref()
        .map(|report| now.saturating_sub(report.at_ms));
    // How long since the pty last produced a new byte. A boundary report's own
    // staleness only *suggests* a dropped follow-up — the harness can still be
    // writing a long reply for the rest of the turn with no further report at
    // all (there is no hook event bracketing "now generating the answer"), and
    // that reply routinely prints nothing shaped like Claude's spinner. Freshly
    // arriving output is what tells the two apart: a session that has gone
    // quiet on screen *and* stopped producing bytes is what a dropped report
    // looks like; one still streaming new content plainly is not, however old
    // its last report is. The same signal is also the backstop for an
    // abandoned *open-ended* report — one whose closing event never
    // arrives — bounding trust in it to [`OPEN_ENDED_ABANDON_MS`] of total
    // silence rather than none at all.
    //
    // Documented, accepted hole: `last_output_at` cannot tell the harness's own
    // progress apart from an operator typing at an idle composer. A raw-mode
    // TUI echoes keystrokes itself — repainting its input line with the new
    // character — so every keystroke is genuine pty output the reader thread
    // sees, indistinguishable here from real work (see
    // `an_operators_own_keystrokes_can_hold_a_stale_report_working`). In the
    // exact scenario the staleness grace exists for — a dropped `Stop`, a
    // boundary report stuck as the session's last — an operator drafting their
    // next message can therefore hold the row `working` for as long as they
    // keep typing, and for up to another [`HOOK_STALE_GRACE_MS`] after their
    // last keystroke. This is judged narrow enough to accept rather than build
    // a second mechanism for: it requires the rare dropped-report case to
    // *coincide* with active typing, it is purely cosmetic (the composer and
    // submission are unaffected), and it self-corrects the moment either typing
    // stops for a full grace window or the next real turn files its own
    // `UserPromptSubmit`.
    let output_quiet_ms = now.saturating_sub(session.last_output_at());
    let screen_working = attention::is_working(&contents);
    let working = working_state(
        screen_working,
        hook_active,
        report_age_ms,
        report_open_ended,
        Some(output_quiet_ms),
    );
    let rang = eligible_bells > 0 && !working;
    // Screen first — a permission menu the scraper can read is more specific
    // than the generic "waiting" a Notification report names — then the hook,
    // which is the harness saying so itself and the one cue a reworded prompt
    // or a full-screen TUI cannot defeat — then the bell, the vaguest cue,
    // which loses to anything named.
    let hook = hook_log.and_then(|log| {
        attention::hook_attention(session.provider(), session.grant_session(), working, log)
    });
    let cue = attention::detect(session.provider(), &contents)
        .or(hook)
        .or_else(|| rang.then(|| attention::bell_cue(session.provider())));
    // Working and waiting are mutually exclusive session states. The screen
    // detector can recognise a permission menu before Claude's Notification
    // hook arrives, while the previous active-turn hook is still last in the
    // log; the concrete cue wins that short race.
    let working = working && cue.is_none();

    let mut state = lock(&session.attention);
    // Release or acknowledgement raced this sample; it owns the newer truth.
    if state.generation != generation {
        return;
    }
    // Which of the three states behind `working` this poll is in decides
    // whether the bell is accounted now or held — see `bells_are_deferred`.
    //
    // The debt consumption and the bell watermark are made as one decision,
    // not two, because a bell sampled while deferring must stay genuinely
    // *unaccounted* rather than half-accounted: settling
    // `pending_completion_bells` against it now while leaving `seen_bells`
    // behind would spend the debt on a bell that never produced a cue, and
    // once `working` clears there would be no debt left to absorb the same
    // bell when it finally becomes eligible — the exact spurious-cue bug this
    // deferral exists to avoid, just moved from the watermark onto the debt
    // instead.
    //
    // Leaving the debt untouched here does not extend its life, though:
    // `completion_deadline`'s expiry check above runs against real wall-clock
    // time on *every* poll regardless of deferral, so a promise past its grace
    // is force-cleared to zero on schedule whether or not this poll defers.
    // Deferring only postpones *deciding* whether the debt absorbs this bell
    // until `working` finally clears — by which point
    // `pending_completion_bells` already reflects whatever that expiry logic
    // did to it in the meantime, so the eventual decision is the one this poll
    // would have made, taken later.
    let deferring = bells_are_deferred(working, screen_working, report_open_ended);
    let (consumed_pending, new_seen_bells) = bell_accounting(
        deferring,
        unseen_bells,
        pending_completion_bells,
        state.seen_bells,
        bells,
    );
    state.pending_completion_bells -= consumed_pending;
    if state.pending_completion_bells == 0 {
        state.completion_deadline = None;
    }
    state.seen_bells = new_seen_bells;
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

/// Whether this poll must leave a bell unaccounted rather than deciding it now.
///
/// Three states hide behind a `working` of `true`, and only one of them defers:
///
/// - **the screen shows it** (`screen_working`): a spinner or working marker is
///   on the pane. A bell here is a progress chime and is accounted at once,
///   exactly as it always was.
/// - **an open-ended report is being trusted** (`report_open_ended`): a tool,
///   subagent, or compaction has started and not returned. The harness is
///   *demonstrably* working — its own report says the operation is still open —
///   and the screen showing no spinner is expected, because what is on it is
///   the tool's output rather than Claude's chrome. A bell rung in that phase
///   (a build that prints one, a shell beeping at an error) is a progress chime
///   too, and deferring it would resurrect it as a false "wants you" once the
///   turn ends.
/// - **a boundary report is trusted through a corroboration gap**: everything
///   else. Here `working` rests on a report that may already have lost its
///   `Stop`, and the bell may be the completion chime for a turn that is over.
///   That one is worth holding, so it can still become the cue the operator is
///   owed once `working` clears.
///
/// Keying on `screen_working` alone collapsed the middle case into the last
/// one, which is the whole reason this predicate is named rather than inlined.
pub(super) fn bells_are_deferred(
    working: bool,
    screen_working: bool,
    report_open_ended: bool,
) -> bool {
    working && !screen_working && !report_open_ended
}

/// One poll's bell accounting: how much of the promised-completion debt this
/// sample consumes, and what the bell watermark becomes.
///
/// Made as a single decision rather than two independent ones. `refresh`
/// defers a bell — leaves it unseen — exactly when `working` is true *solely*
/// because a stale-tolerant hook report says so while the screen does not
/// corroborate it; a bell sampled in that window must not be silently spent
/// against the completion debt (`pending_completion_bells`) while its
/// watermark (`seen_bells`) is left behind, or the debt would be gone by the
/// time `working` finally clears with nothing left unseen to absorb the same
/// bell — the exact spurious-cue bug this deferral exists to avoid, just
/// moved from the watermark onto the debt instead. A poll that is not
/// deferring accounts for both exactly as it always did.
///
/// Returns `(consumed_pending, new_seen_bells)`.
pub(super) fn bell_accounting(
    deferring: bool,
    unseen_bells: usize,
    pending_completion_bells: usize,
    seen_bells: usize,
    bells: usize,
) -> (usize, usize) {
    if deferring {
        (0, seen_bells)
    } else {
        let consumed_pending = unseen_bells.min(pending_completion_bells);
        (consumed_pending, consumed_bell_count(seen_bells, bells))
    }
}

/// Resolve working state from the screen's live edges and hook-backed middle.
///
/// `screen_working` (a visible spinner or working marker) starts work, and the
/// harness's own lifecycle reports carry it through every paint that has no
/// spinner on it — a tool result streaming in, the reply being written out —
/// without depending on Claude's changing progress vocabulary. With no
/// report, this reduces to the screen alone.
///
/// The order changed with Claude 2.1: a restored composer used to end the turn
/// ahead of the reports, on the reasoning that a harness takes its prompt away
/// while it works. Claude does not. It keeps the composer drawn, framed and
/// typeable from the first token to the last, so "a composer is on screen"
/// stopped meaning "the turn is over" — and as the ordinary state of a working
/// pane, it overruled the reports on every sample and left the rail claiming
/// every busy Claude was idle. The reports now win that middle; the ordinary
/// cost is that a turn can hold `working` for the moment between the composer
/// settling and `Stop` being filed, which is one hook dispatch and
/// self-correcting.
///
/// `hook_active`, `report_age_ms`, and `output_quiet_ms` are read from a
/// single hook-log snapshot by the caller (`refresh`) rather than derived here
/// from a fresh, separately-timed read — two reads a report apart could
/// otherwise pair a stale age with a report that just renewed.
///
/// That trust is bounded by `report_age_ms`, though: `commands/hook.rs`'s
/// `run_hook_cmd` deliberately swallows a report's delivery failure or
/// timeout, so a session whose `Stop` report never arrived would otherwise
/// hold `working` for the rest of its life once an idle screen stopped ending
/// a turn on its own. A report older than [`HOOK_STALE_GRACE_MS`] — comfortably
/// longer than one hook dispatch — is presumed dropped rather than trusted,
/// and the idle screen wins instead. Ageing the *report*, not the screen's
/// idle streak, is what lets a session resume cleanly: a fresh report right
/// after a long-idle prompt is trusted immediately, rather than inheriting a
/// staleness clock that had nothing to do with it.
///
/// The fast clock is not applied to every report, though: `report_open_ended`
/// (`hook_report_is_open_ended`: a tool, subagent, or compaction that has
/// started and not yet returned) exempts one from it entirely once it is no
/// longer fresh. A slow build under `Bash`, or a long-running subagent, can
/// legitimately sit on `PreToolUse`/`SubagentStart` far longer than
/// [`HOOK_STALE_GRACE_MS`] with no visible spinner of its own — the tool's own
/// output is what is on screen, not Claude's chrome — and that is still
/// genuine work, not a dropped report. Only a report marking a *boundary* just
/// crossed (`PostToolUse`, `SubagentStop`, `UserPromptSubmit`, `PostCompact`)
/// is held to the fast clock once stale, because visible activity is expected
/// again within moments of one of those, and a long silence after one is what
/// a dropped follow-up report looks like.
///
/// `report_age_ms` and `report_open_ended` are deliberately two separate
/// facts rather than one collapsed into the other (an *aged* `report_age_ms`
/// used to read `None` for an open-ended report, "no age to check"). A report
/// can be both open-ended *and* fresh — a `PreToolUse` that just started after
/// ten minutes of silence — and the two facts pull in different directions:
/// its youth is direct evidence of life on its own, while its open-endedness
/// is what should keep it trusted *once it is no longer young*. Folding one
/// into the other picks only one of those and loses the other; kept apart,
/// the order below can honour both.
///
/// An open-ended report is not trusted with *no* bound at all, though — see
/// [`OPEN_ENDED_ABANDON_MS`]. If its own closing event is the one that got
/// lost, nothing would otherwise ever reconsider it, which is the same
/// dropped-report failure the fast clock exists to catch, just on the opening
/// side instead of the closing one.
///
/// Even a boundary report's staleness is only *suggestive*, though, not
/// conclusive on its own: there is no hook event bracketing "now writing the
/// reply", so a `PostToolUse` or `UserPromptSubmit` can legitimately remain
/// the last report for as long as a long answer takes to generate — and that
/// phase is one of the spinner-less paints this whole function exists to
/// cover (see the module-level Claude 2.1 note above), so the screen alone
/// cannot corroborate it either. `output_quiet_ms` — how long since the pty
/// last produced a new byte — is what tells a genuinely dropped report apart
/// from a merely-quiet-on-the-recognised-markers one: a session that has gone
/// both unrecognisable *and* silent is what a dropped report looks like: one
/// still streaming fresh content, however old its last report, plainly is
/// not.
///
/// Documented, accepted hole (see `refresh`'s computation of
/// `output_quiet_ms` for the full accounting): a raw-mode TUI echoes
/// keystrokes by repainting its own input line, so an operator typing at an
/// idle composer produces genuine pty output indistinguishable here from the
/// harness's own progress. Narrow in practice — it needs a dropped report
/// (already rare) to coincide with active typing, is purely cosmetic, and
/// self-corrects within one grace window of the last keystroke or the next
/// real `UserPromptSubmit`.
pub(super) fn working_state(
    screen_working: bool,
    hook_active: Option<bool>,
    report_age_ms: Option<i64>,
    report_open_ended: bool,
    output_quiet_ms: Option<i64>,
) -> bool {
    if screen_working {
        return true;
    }
    let Some(active) = hook_active else {
        // No lifecycle report to consult, and no spinner on screen. A harness
        // that is genuinely mid-turn is indistinguishable here from one
        // resting at its prompt, and a row that spins at nothing is the
        // worse of the two mistakes.
        return false;
    };
    if !active {
        return false;
    }
    // A *known* fresh age is checked first, ahead of everything else,
    // regardless of open-endedness: a report recorded moments ago is direct
    // evidence of life on its own, and silence that ended before the report
    // was even filed says nothing about whether the report itself is stale.
    // Without this, a session silent for `OPEN_ENDED_ABANDON_MS` that then
    // gets a brand-new open-ended report — a fresh `PreToolUse` — would read
    // idle until the tool's first byte, even though the report already proved
    // the session alive.
    if report_age_ms.is_some_and(|age| age < HOOK_STALE_GRACE_MS) {
        return true;
    }
    // The backstop: however open-ended the report, total pty silence this
    // long means abandoned, not merely between visible beats. Checked next —
    // after a known-fresh age, but before the open-ended exemption below —
    // because it must still override an open-ended report whose *own*
    // closing event was lost; that is the property this backstop exists for,
    // and a fresh report (of either kind) is the only thing allowed to
    // outrank it.
    if output_quiet_ms.is_some_and(|quiet| quiet >= OPEN_ENDED_ABANDON_MS) {
        return false;
    }
    // Open-ended and past its fresh window, but not past the backstop either:
    // exactly the ordinary case a slow build sits in for most of its life,
    // and the whole reason `report_open_ended` exists as its own fact rather
    // than as an absent age.
    if report_open_ended {
        return true;
    }
    // A *stale* boundary report reaches here (a known age that failed the
    // freshness check above, and not open-ended). Fresh output still
    // corroborates continued work even though nothing on it matched a
    // recognised marker; unknown quietness (no measurement) is treated the
    // same way as an unknown age above — trusted rather than presumed stale.
    output_quiet_ms.is_none_or(|quiet| quiet < HOOK_STALE_GRACE_MS)
}
