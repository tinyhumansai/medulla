//! Keeping each session's "does this harness want me" flag current.
//!
//! The classification itself is [`super::super::attention`]; this is the part
//! that has to hold a lock, watch a clock, and decide when a cue is answered.
//!
//! Three rules shape it:
//!
//! - **Refreshed from a timer thread**, on a 200 ms interval. Attention is a
//!   property of the screen at rest: a harness paints its permission prompt and
//!   then goes silent, which is the state being detected. Sampling from the
//!   reader thread would only catch frames while bytes are arriving, missing the
//!   final frame that carries the question. A timer avoids that race. The reader
//!   thread can call [`Self::refresh_attention`] directly but is throttled so a
//!   fast harness does not turn a heuristic into a hot loop.
//! - **A named cue outranks a vague one, and an incumbent keeps its clock.** A
//!   permission prompt repainted every frame must read as one prompt that has
//!   been up for four minutes, not as a fresh one every frame — that elapsed
//!   time is the thing an operator actually acts on.
//! - **Answering it clears it.** Attention is a request, and a request that
//!   survives being answered is worse than none: the rail would blink at a
//!   harness that has already moved on, which is how an indicator gets ignored.

use std::sync::{Arc, Weak};

use super::super::attention::{self, AttentionKind, HarnessAttention};
use super::session::consumed_bell_count;
use super::PtyManager;

/// How often a session's screen is reclassified.
///
/// Fast enough that a prompt blinks about as soon as the operator could have
/// seen it painted, slow enough that a harness redrawing at full speed does not
/// turn a heuristic into a hot loop.
const ATTENTION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// The floor between two classifications, for the throttle that guards a direct
/// call. Deliberately under [`ATTENTION_INTERVAL`], so a poll that wakes on time
/// is never throttled out by its own period.
const ATTENTION_INTERVAL_MS: i64 = (ATTENTION_INTERVAL.as_millis() as i64) * 3 / 4;

impl PtyManager {
    /// Read the emulator's current bell counter without holding the sessions
    /// lock while the screen is locked.
    pub(super) fn current_bell_count(&self, id: &str) -> Option<usize> {
        let screen = {
            let sessions = self.inner.sessions.lock().unwrap();
            sessions
                .iter()
                .find(|s| s.row.id == id)
                .map(|s| s.screen.clone())
        }?;
        let parser = screen.lock().unwrap();
        Some(parser.screen().audible_bell_count())
    }

    /// Sample `id`'s screen on a timer for as long as the session lives.
    ///
    /// A timer rather than the reader thread, and that is the whole design
    /// decision here. Attention is a property of the screen **at rest**: a
    /// harness paints its permission prompt and then goes completely silent,
    /// which is exactly the state being detected. Classifying from the reader
    /// means classifying only while bytes are arriving — so the final frame,
    /// the one carrying the question, is the one frame never looked at. That is
    /// not a rare race; it is every prompt.
    ///
    /// Holds a [`Weak`] on the manager, so this thread cannot keep the sessions
    /// (and their children) alive past the last real handle: `Inner`'s `Drop`
    /// is what kills the harnesses, and a poller with a strong reference would
    /// quietly disable it.
    pub(super) fn spawn_attention_poller(&self, id: String) {
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || loop {
            std::thread::sleep(ATTENTION_INTERVAL);
            let Some(inner) = Weak::upgrade(&weak) else {
                return; // the manager is gone, and with it the session
            };
            let manager = PtyManager { inner };
            // An exited session's last screen stays readable, but nobody can
            // answer what it was asking, so there is nothing left to watch.
            match manager.row(&id) {
                Some(row) if row.state.is_running() => manager.refresh_attention(&id),
                _ => return,
            }
        });
    }

    /// Reclassify `id`'s screen and update its cue, at most every
    /// [`ATTENTION_INTERVAL_MS`].
    ///
    /// Primary caller is the timer thread spawned by [`Self::spawn_attention_poller`],
    /// which runs every [`ATTENTION_INTERVAL`]. The reader thread may call this
    /// directly but is throttled so fast redraws do not turn classification into
    /// a hot loop. Cheap when throttled out: one lock, one comparison.
    pub(super) fn refresh_attention(&self, id: &str) {
        let now = self.now();
        // Everything needed off the session is taken under the lock and the
        // guard dropped before the screen is read, so classification — which
        // walks the whole grid — never holds up a render frame.
        let taken = {
            let mut sessions = self.inner.sessions.lock().unwrap();
            let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
                return;
            };
            if now.saturating_sub(session.attention_checked_at) < ATTENTION_INTERVAL_MS {
                return;
            }
            session.attention_checked_at = now;
            (
                session.screen.clone(),
                session.row.provider,
                session.seen_bells,
                session.attention_generation,
            )
        };
        let (screen, provider, seen_bells, attention_generation) = taken;

        let (contents, bells) = {
            let parser = screen.lock().unwrap();
            let screen = parser.screen();
            (screen.contents(), screen.audible_bell_count())
        };

        // A bell only counts while the harness is not plainly mid-turn: they all
        // ring one as a tool finishes, and a progress chime is not a question.
        let rang = bells > seen_bells && !attention::is_working(&contents);
        let cue = attention::detect(provider, &contents)
            .or_else(|| rang.then(|| attention::bell_cue(provider)));

        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return;
        };
        // A release can happen while the screen is classified outside the
        // sessions lock. Its generation bump means this sample belongs to the
        // completed turn and must not put that turn's bell back on the row.
        if session.attention_generation != attention_generation {
            return;
        }
        // Recorded whether or not it produced a cue, so one ring is never
        // counted twice.
        session.seen_bells = bells;
        session.row.attention = match (cue, session.row.attention.take()) {
            (None, held) => {
                // A screen that says nothing does not clear a bell: the ring is
                // the whole of that signal, and there is no second frame of it
                // to keep the flag alive. Named cues *do* clear, because the
                // prompt leaving the screen is the harness saying it was
                // answered.
                held.filter(|held| held.kind == AttentionKind::Bell)
            }
            (Some((kind, what)), None) => Some(HarnessAttention::new(kind, what, now)),
            (Some((kind, what)), Some(held)) => {
                let fresh = HarnessAttention::new(kind, what, now);
                // Same cue as last time: keep the incumbent, and with it the
                // clock the rail reports.
                if fresh.supersedes(&held) || fresh.what != held.what {
                    Some(fresh)
                } else {
                    Some(held)
                }
            }
        };
    }

    /// Drop `id`'s cue because it has been answered.
    ///
    /// Called when the operator attaches to a pane and when a prompt is injected
    /// — both mean somebody is now dealing with it. Named cues come back on the
    /// next refresh if the harness is in fact still asking, so this is safe to
    /// call optimistically; a bell does not, which is the point.
    ///
    /// Returns whether there was anything to clear.
    pub fn acknowledge(&self, id: &str) -> bool {
        let bells = self.current_bell_count(id);
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return false;
        };
        if let Some(bells) = bells {
            session.seen_bells = consumed_bell_count(session.seen_bells, bells);
        }
        session.attention_generation = session.attention_generation.wrapping_add(1);
        session.row.attention.take().is_some()
    }

    /// What `id` is waiting on the operator for, if anything.
    pub fn attention(&self, id: &str) -> Option<HarnessAttention> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.row.id == id)
            .and_then(|s| s.row.attention.clone())
    }

    /// How many live sessions are waiting on the operator.
    ///
    /// The number the chrome puts on the Agents tab, so a blocked harness is
    /// visible from a tab that is not showing it.
    pub fn waiting_count(&self) -> usize {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.row.state.is_running() && s.row.attention.is_some())
            .count()
    }

    /// All live session IDs that are waiting on the operator.
    ///
    /// Collects the waiting set in one locked pass, so render code can check
    /// membership instead of acquiring a lock per lane.
    pub fn waiting_sessions(&self) -> std::collections::HashSet<String> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.row.state.is_running() && s.row.attention.is_some())
            .map(|s| s.row.id.clone())
            .collect()
    }
}
