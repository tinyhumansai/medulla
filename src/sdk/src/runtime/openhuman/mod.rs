//! [`Runtime`] backed by the embedded OpenHuman core.
//!
//! A fourth [`Runtime`] implementation alongside `backend` (HTTP/SSE), `core`
//! (NDJSON socket) and `mock`. It is the one this migration is heading toward:
//! rather than speaking to a remote process, it drives an OpenHuman core
//! running in-process through the typed embed facade.
//!
//! # Why a `Runtime` impl rather than replacing the trait
//!
//! [`Runtime`] is not "the backend abstraction" — it is the *render-snapshot*
//! contract. Every surface depends on two things,
//! [`snapshot`](Runtime::snapshot) and [`subscribe`](Runtime::subscribe), and
//! [`RuntimeSnapshot`] is a fold of an event stream. That fold and the code
//! rendering it do not care where the events came from. Keeping the trait is
//! also what keeps `MockRuntime` viable, and with it the TUI test suites that
//! drive the whole UI with no backend at all.
//!
//! # Layout
//!
//! [`fold`] is pure translation, [`cell`] is the snapshot plus its notification
//! channel, and this module is the thin part that needs a core. The split is
//! what lets the contract be unit-tested: the core uses process globals and
//! cannot be built per test.
//!
//! # Current scope
//!
//! Reads fold live roster and session data. Submit, abort and new-session drive
//! the backend through the facade. What remains unwired is chat-tree resume —
//! `medulla_chat` lives in the core but has no RPC surface yet — and the event
//! stream, so a submitted turn is accepted but its reply does not yet flow back
//! into the snapshot.
//!
//! The still-unwired paths return a typed error rather than doing nothing. A
//! silent no-op reads as a hung backend and sends someone debugging the
//! network; an explicit error names the layer that is actually missing.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::future::BoxFuture;
use openhuman_core::embed::{Core, CoreError};
use tokio::sync::broadcast;

use super::types::{ContextItem, RuntimeSnapshot, StreamState};

/// Consecutive poll failures tolerated before the header reports `Stalled`.
///
/// At [`POLL_IDLE`] the delay has backed off to one second, so this is roughly
/// five seconds of silence — long enough to ride out a restart, short enough
/// that an operator is not left reading a stale screen.
const STALLED_AFTER: usize = 5;

/// Poll delay while a turn is actively producing events.
const POLL_ACTIVE: std::time::Duration = std::time::Duration::from_millis(120);

/// Ceiling the poll delay backs off to once a session goes quiet.
const POLL_IDLE: std::time::Duration = std::time::Duration::from_millis(1_000);
use super::Runtime;
use crate::ui::chat_store::MainChatSummary;

pub mod cell;
mod feedback;
pub mod fold;
mod worker_ops;

#[cfg(test)]
mod tests;

pub use cell::SnapshotCell;

/// Message for every drive method not yet migrated.
///
/// One constant so the wording cannot drift between call sites, and so tests
/// assert a class of failure rather than prose.
pub const NOT_YET_WIRED: &str =
    "this action is not wired to the embedded core yet (read-only while the migration lands)";

/// A [`Runtime`] driving an in-process OpenHuman core.
pub struct OpenHumanRuntime {
    core: Arc<Core>,
    cell: SnapshotCell,
    /// The session `submit`/`abort` act on.
    ///
    /// Minted lazily on first submit rather than at construction: booting must
    /// not create a durable session on the backend for a host that only ever
    /// reads, and a failed boot should leave no trace behind.
    session: SessionSlot,
    /// Replay cursor: the highest event `seq` already folded in.
    ///
    /// Shared so the poll loop can advance it from its own task. Starts at
    /// `None`, which the backend reads as "replay from the beginning".
    cursor: Arc<tokio::sync::Mutex<Option<i64>>>,
    /// Consecutive failed polls, for [`stream_state`](Runtime::stream_state).
    ///
    /// An `AtomicUsize` rather than a lock: it is written from the poll task and
    /// read from the render path, which must never block on the poller.
    poll_failures: Arc<std::sync::atomic::AtomicUsize>,
    /// The tiny.place hub handle, once the hub has connected.
    ///
    /// The worker surface — the roster, its activity, the watched screens and
    /// every mutation — is the hub's, not the core's: tiny.place peers are this
    /// device's business and never reach the orchestration backend. Without it
    /// the Workers tab reads empty and, worse, its mutations inherit the trait's
    /// no-op success.
    hub: HubSlot,
    /// The backend deployment this host is configured for, when one is known.
    ///
    /// Only the feedback board needs it: the board lives on the cloud backend
    /// rather than in the core, so those calls mint a short-lived
    /// [`MedullaClient`](crate::client::MedullaClient) against this URL with the
    /// core's stored session. `None` — an unconfigured host — leaves the board
    /// reporting itself unavailable rather than dialling a default deployment
    /// the operator never signed in to.
    backend_base_url: Option<String>,
}

/// Shared slot the hub fills with its handle once connected.
///
/// A `std::sync::Mutex` because every read happens on the render path, which
/// must not await; the guard is never held across one.
pub type HubSlot = Arc<std::sync::Mutex<Option<crate::hub::HubHandle>>>;

/// Shared handle to the active session id.
///
/// An `Arc` because `submit` and `abort` move it into `'static` futures — the
/// trait's futures outlive the borrow of `&self`.
type SessionSlot = Arc<tokio::sync::Mutex<Option<String>>>;

impl OpenHumanRuntime {
    /// Wrap an already-booted core.
    ///
    /// Performs no I/O — call [`refresh`](Self::refresh) for that. Boot and
    /// first fetch are separate so a host can paint its UI before the first
    /// round trip lands.
    pub fn new(core: Arc<Core>) -> Self {
        Self::with_hub(core, HubSlot::default())
    }

    /// Wrap a core and share the slot the tiny.place hub fills once connected.
    ///
    /// The slot is passed empty at construction and filled later: the hub needs
    /// a signed-in session to connect, which is only established after the
    /// runtime exists.
    pub fn with_hub(core: Arc<Core>, hub: HubSlot) -> Self {
        Self {
            core,
            cell: SnapshotCell::new(),
            session: SessionSlot::default(),
            cursor: Arc::new(tokio::sync::Mutex::new(None)),
            poll_failures: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            hub,
            backend_base_url: None,
        }
    }

    /// Point the backend-facing surfaces (today: the feedback board) at the
    /// deployment this host is configured for.
    ///
    /// The URL comes from the caller's loaded config rather than the core: the
    /// core resolves the same deployment by construction but exposes no RPC for
    /// the URL it settled on.
    #[must_use]
    pub fn with_backend_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.backend_base_url = Some(base_url.into());
        self
    }

    /// The hub handle, if one has connected.
    ///
    /// Cloned out rather than borrowed so no caller holds the lock across an
    /// await — the render path reads this slot on every frame.
    fn hub(&self) -> Option<crate::hub::HubHandle> {
        self.hub.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A clone of the shared session slot, for moving into a `'static` future.
    fn session_handle(&self) -> SessionSlot {
        Arc::clone(&self.session)
    }

    /// The active session, minting one if there is none yet.
    ///
    /// The lock is held across the mint so two concurrent submits cannot each
    /// create a session and leave one orphaned on the backend. Static rather
    /// than a method because the trait's futures are `'static` and cannot
    /// borrow `&self`.
    async fn session_for(core: &Core, slot: &SessionSlot) -> Result<String, CoreError> {
        let mut guard = slot.lock().await;
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
        let created = core.medulla().create_session(None).await?;
        tracing::debug!("[openhuman_runtime] minted session {}", created.session_id);
        *guard = Some(created.session_id.clone());
        Ok(created.session_id)
    }

    /// Pull the roster and session list, fold them in, and notify.
    ///
    /// Best-effort by design: an unconfigured or signed-out host is an expected
    /// state rather than a failure, so a rejected call leaves the previous
    /// snapshot intact instead of blanking the UI.
    pub async fn refresh(&self) {
        let medulla = self.core.medulla();

        let roster = match medulla.roster().await {
            Ok(workers) => fold::roster(workers),
            Err(err) => {
                tracing::debug!("[openhuman_runtime] roster unavailable: {err}");
                Vec::new()
            }
        };
        let threads = match medulla.list_sessions().await {
            Ok(sessions) => fold::threads(sessions),
            Err(err) => {
                tracing::debug!("[openhuman_runtime] sessions unavailable: {err}");
                Vec::new()
            }
        };

        tracing::debug!(
            "[openhuman_runtime] refresh roster={} threads={}",
            roster.len(),
            threads.len()
        );
        // Adopt the first thread when nothing is selected yet. The UI falls back
        // to rendering index 0 as active regardless, so leaving the slot empty
        // does not mean "nothing selected" on screen — it means the header names
        // a conversation that `submit` would not send to and `poll_events` would
        // not load. Only on the way *in*: a later refresh must never move the
        // operator's own selection.
        let adopt = self
            .cell
            .active_thread_id()
            .is_empty()
            .then(|| threads.first().map(|t| t.id.clone()))
            .flatten();
        self.cell.apply(roster, threads);
        if let Some(id) = adopt {
            tracing::debug!("[openhuman_runtime] adopting initial session {id}");
            self.cell.set_active_thread(id.clone());
            *self.session.lock().await = Some(id);
        }
    }

    /// Fetch events past the cursor and fold them into the snapshot.
    ///
    /// Returns the number folded, so a caller can back off when a stream is
    /// idle instead of polling at a fixed rate regardless of traffic.
    ///
    /// Best-effort like [`refresh`](Self::refresh): a rejected fetch leaves the
    /// snapshot and the cursor untouched rather than blanking the transcript or
    /// replaying it from the start.
    pub async fn poll_events(&self) -> usize {
        let Some(session) = self.session.lock().await.clone() else {
            return 0;
        };
        let after = *self.cursor.lock().await;

        let fetched = match self.core.medulla().list_events(&session, after).await {
            Ok(events) => events,
            Err(err) => {
                // Count it rather than only logging. A backend that is
                // unreachable otherwise leaves the transcript inert forever
                // with no signal beyond a debug line nobody is watching.
                let n = self.poll_failures.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!("[openhuman_runtime] event poll failed ({n}): {err}");
                return 0;
            }
        };
        self.poll_failures.store(0, Ordering::Relaxed);

        // The operator can switch threads while a fetch is in flight. Folding
        // the outgoing thread's batch into the incoming thread's transcript
        // would render one conversation inside another, so a batch that
        // outlived its session is dropped; the new session replays from its own
        // cursor on the next tick.
        if self.session.lock().await.as_deref() != Some(session.as_str()) {
            tracing::debug!("[openhuman_runtime] dropping a batch from a superseded session");
            return 0;
        }

        let events = fold::events(fetched);
        if events.is_empty() {
            return 0;
        }

        // Advance the cursor BEFORE folding, so a panic mid-fold cannot leave
        // the cursor behind and replay the same batch forever.
        if let Some(max) = fold::max_seq(&events) {
            *self.cursor.lock().await = Some(max as i64);
        }
        let count = events.len();
        tracing::debug!("[openhuman_runtime] folded {count} events after {after:?}");
        // Liveness comes from the batch's cycle boundaries, not from the fact
        // that a batch arrived at all. A batch with no boundary leaves the flag
        // alone, so the spinner neither flickers off between batches nor keeps
        // turning after the `cycle_end` that ended the turn.
        let running = fold::running_after(&events);
        self.cell.append_events(events, running);
        count
    }

    /// Drive [`poll_events`](Self::poll_events) in the background until dropped.
    ///
    /// Returns immediately; the caller keeps the returned handle only if it
    /// wants to stop the loop early. Takes `Arc<Self>` because the loop outlives
    /// the call and the runtime is already shared with the UI.
    ///
    /// The cadence adapts: a batch means the turn is producing, so poll again
    /// promptly; an empty fetch backs off toward `POLL_IDLE`. A fixed fast
    /// tick would spend most of its life querying an idle session, and a fixed
    /// slow one would make streaming replies arrive in visible steps.
    pub fn spawn_poll_loop(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let rt = Arc::clone(self);
        tokio::spawn(async move {
            let mut delay = POLL_IDLE;
            loop {
                tokio::time::sleep(delay).await;
                delay = if rt.poll_events().await > 0 {
                    POLL_ACTIVE
                } else {
                    // Ease back rather than snapping to idle: a turn often has
                    // gaps between batches, and snapping would add the full
                    // idle delay to the very next token.
                    (delay * 2).min(POLL_IDLE)
                };
            }
        })
    }

    /// The facade, for callers needing something the trait does not model.
    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }
}

impl Runtime for OpenHumanRuntime {
    fn describe(&self) -> String {
        "openhuman (embedded core)".to_string()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        self.cell.snapshot()
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.cell.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        let session = self.session_handle();
        Box::pin(async move {
            let id = Self::session_for(&core, &session).await?;
            // Non-blocking: the reply arrives over the event stream, so the UI
            // can render progress instead of freezing until the turn finishes.
            core.medulla().send_message(&id, &input, false).await?;
            Ok(())
        })
    }

    fn steering_reaches_backend(&self) -> bool {
        // The core models a session abort but has no RPC for answering a
        // question or cancelling one delegated task, so both would be silent
        // no-ops. Saying so lets the UI report the gap instead of announcing an
        // answer that was never delivered.
        false
    }

    fn submit_settles_cycle(&self) -> bool {
        // `send_message(.., false)` returns on acceptance; the reply arrives
        // over the polled event stream. Reporting completion here would tell
        // the operator the turn is done while it is still producing.
        false
    }

    fn stream_state(&self) -> Option<StreamState> {
        // Derived from real poll health, not hardcoded. `Live` is honest here
        // even though this polls rather than streams: the glyph answers "is
        // state arriving?", and a succeeding poll means it is.
        //
        // The threshold is >1 rather than >0 so a single blip — a restart, a
        // dropped connection — does not flap the header on and off.
        Some(fold::stream_state(
            self.poll_failures.load(Ordering::Relaxed),
            STALLED_AFTER,
        ))
    }

    fn abort(&self) {
        let core = Arc::clone(&self.core);
        let session = self.session_handle();
        // Fire-and-forget: the trait is sync, and an abort that has not landed
        // yet is still better than blocking the UI thread on a round trip.
        tokio::spawn(async move {
            let Some(id) = session.lock().await.clone() else {
                tracing::debug!("[openhuman_runtime] abort with no active session");
                return;
            };
            if let Err(err) = core.medulla().abort(&id).await {
                tracing::debug!("[openhuman_runtime] abort failed: {err}");
            }
        });
    }

    fn workers(&self) -> Vec<crate::runtime::WorkerInfo> {
        let Some(hub) = self.hub() else {
            return Vec::new();
        };
        hub.list()
            .into_iter()
            .map(|worker| {
                let details = hub.system_info(&worker.id);
                worker_ops::hub_worker_to_info(worker, details)
            })
            .collect()
    }

    fn worker_activity(&self) -> Vec<crate::hub::WorkerActivity> {
        self.hub()
            .map(|hub| hub.activity().snapshot())
            .unwrap_or_default()
    }

    fn worker_screens(&self) -> Vec<crate::hub::WatchedScreen> {
        self.hub()
            .map(|hub| hub.screens().snapshot())
            .unwrap_or_default()
    }

    fn watch_task(
        &self,
        worker: String,
        task_id: String,
        watch: bool,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let hub = self.hub();
        Box::pin(async move {
            let Some(hub) = hub else {
                // Reported rather than swallowed. A silent success here is the
                // worst version of this failure: the pane stays empty, the
                // worker is never asked for anything, and nothing anywhere says
                // why. Stopping a watch is still a no-op — there is genuinely
                // nothing to stop — so only the request to *start* one is an
                // error worth surfacing.
                return if watch {
                    Err(anyhow::anyhow!(
                        "no hub is connected, so this worker cannot be asked to stream its screen"
                    ))
                } else {
                    Ok(())
                };
            };
            let result = if watch {
                hub.watch(&worker, &task_id).await
            } else {
                hub.unwatch(&worker, &task_id).await
            };
            result.map_err(|e| anyhow::anyhow!(e))
        })
    }

    fn worker_op(&self, op: crate::runtime::WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        let hub = self.hub();
        Box::pin(async move {
            match hub {
                Some(hub) => worker_ops::apply_worker_op(&hub, op).await,
                // Reading an empty roster is honest; silently succeeding at a
                // *mutation* that did not happen is not. Without a hub there is
                // nothing to add a worker to, and reporting "updated" leaves the
                // operator watching for a peer that was never registered.
                None => Err(anyhow::anyhow!(
                    "no orchestrator hub is attached — sign in and restart, or set \
                     MEDULLA_HUB_WORKERS"
                )),
            }
        })
    }

    fn logout(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        // Through the core, not a file: it owns the keychain entry as well as
        // the profile on disk, so anything less leaves the real credential in
        // place and the next start signed in again.
        Box::pin(async move {
            crate::core_host::auth::clear_session(&core).await?;
            Ok(())
        })
    }

    fn new_session(&self) {
        let session = self.session_handle();
        let cursor = Arc::clone(&self.cursor);
        // The transcript is per-session, so it goes with the session slot. Left
        // behind, the old rows would sit under a brand-new conversation and the
        // stale cursor would make the new session's opening events look already
        // seen — the same coupling `set_active_thread` maintains, in reverse.
        self.cell.switch_thread(String::new());
        // Clear rather than mint: the next submit mints one. Minting here would
        // create a durable session the operator may never use.
        tokio::spawn(async move {
            *session.lock().await = None;
            *cursor.lock().await = None;
            tracing::debug!("[openhuman_runtime] active session cleared");
        });
    }

    fn set_active_thread(&self, id: String) {
        // Switching the *display* selection without switching the session the
        // drive methods act on is how a prompt ends up in the thread the
        // operator just navigated away from, under a header naming the one they
        // chose. The two move together or not at all.
        self.cell.switch_thread(id.clone());
        let session = self.session_handle();
        let cursor = Arc::clone(&self.cursor);
        tokio::spawn(async move {
            // Session first, then cursor: `poll_events` reads them in that
            // order and re-checks the session before folding, so the worst
            // interleaving drops one in-flight batch rather than replaying the
            // outgoing thread's log into the incoming thread's transcript.
            *session.lock().await = Some(id.clone());
            // From the start: the incoming thread's rows were just cleared, so
            // the old cursor would skip the whole transcript the operator
            // switched over to read.
            *cursor.lock().await = None;
            tracing::debug!("[openhuman_runtime] active session switched to {id}");
        });
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        // The chat-tree store lives in the core as `medulla_chat` but has no RPC
        // surface yet. Empty rather than an error: "no saved chats" is a state
        // the Chat tab already renders, whereas an error would show a failure
        // where there is none.
        Box::pin(async { Ok(Vec::new()) })
    }

    fn resume_chat(&self, _main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Err(anyhow::anyhow!(NOT_YET_WIRED)) })
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        // The embedder owns the core's lifecycle; this runtime borrows it.
        // Tearing it down here would leave the process without a core and no
        // way to build a second one — the context is a `OnceLock`, so a relogin
        // could never recover.
        Box::pin(async { Ok(()) })
    }

    // --- feedback board ---------------------------------------------------
    // Served by the cloud backend rather than the core; the bodies live in
    // [`feedback`], which mints a client per call from the core's session.

    fn list_feedback(
        &self,
        query: crate::client::FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<crate::client::FeedbackPage>>> {
        self.board_list(query)
    }

    fn feedback_detail(
        &self,
        id: String,
    ) -> BoxFuture<'static, anyhow::Result<crate::client::FeedbackDetail>> {
        self.board_detail(id)
    }

    fn vote_feedback(
        &self,
        id: String,
        value: i8,
    ) -> BoxFuture<'static, anyhow::Result<crate::client::FeedbackItem>> {
        self.board_vote(id, value)
    }

    fn comment_feedback(
        &self,
        id: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<crate::client::FeedbackComment>> {
        self.board_comment(id, body)
    }

    fn submit_feedback(
        &self,
        kind: crate::client::FeedbackType,
        title: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<crate::client::FeedbackSubmission>> {
        self.board_submit(kind, title, body)
    }
}
