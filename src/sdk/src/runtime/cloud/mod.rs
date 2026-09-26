//! [`Runtime`] backed by the Medulla cloud backend.
//!
//! The [`Runtime`] the product runs on, alongside `mock` (tests and demos). It
//! drives the orchestration API directly through [`MedullaClient`].
//!
//! # Why this stopped going through an embedded core
//!
//! It used to hold an OpenHuman core and reach the backend through that core's
//! `embed::Medulla` facade — which was itself an RPC hop onto the core's own
//! Medulla client, against the same deployment this SDK already had a typed
//! client for. Every call therefore paid a JSON round trip, an error-string
//! decode and a second set of wire types to arrive where
//! [`MedullaClient`] arrives in one hop. Sessions were never local state: they
//! live on the backend, so nothing was gained by asking an in-process core to
//! fetch them.
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
//! channel, and this module is the thin part that needs a client. The split is
//! what lets the contract be unit-tested without a backend.
//!
//! # Current scope
//!
//! Reads fold live roster and session data; submit, abort and new-session drive
//! the backend; the polled event stream folds a turn's output back into the
//! snapshot.
//!
//! Paths the backend has no route for return a typed error rather than doing
//! nothing. A silent no-op reads as a hung backend and sends someone debugging
//! the network; an explicit error names the layer that is actually missing.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::broadcast;

use super::types::{ContextItem, RuntimeSnapshot, StreamState};
use crate::client::{ClientError, MedullaClient};

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

/// How long an accepted turn may produce nothing at all before the running
/// indicator settles.
///
/// The backend records the turn inside the request that accepts it, so its own
/// echo is readable by the next poll — one [`POLL_IDLE`] plus a round trip.
/// Silence an order of magnitude past that is not a slow cycle, it is a turn
/// nothing is working on, and the spinner should stop claiming otherwise. The
/// transcript keeps the turn either way.
const ECHO_STALL_AFTER: std::time::Duration = std::time::Duration::from_secs(15);
use super::Runtime;
use crate::ui::chat_store::MainChatSummary;

pub mod cell;
pub mod connect;
mod feedback;
pub mod fold;
mod worker_ops;

#[cfg(test)]
mod tests;

pub use cell::SnapshotCell;

/// Message for every drive method the backend has no route for.
///
/// One constant so the wording cannot drift between call sites, and so tests
/// assert a class of failure rather than prose.
pub const NOT_YET_WIRED: &str =
    "this action has no orchestration-backend route yet (read-only until one lands)";

/// A [`Runtime`] driving the Medulla orchestration backend.
pub struct CloudRuntime {
    client: Arc<MedullaClient>,
    /// Shared so a submit that failed can settle the turn it drew from inside
    /// the spawned future, which outlives the `&self` that started it.
    cell: Arc<SnapshotCell>,
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
    /// Set by `new_session`, cleared by the next mint.
    ///
    /// `new_session` cannot clear the session slot synchronously — it is an
    /// async mutex and the trait method is not async — so it spawned a task to
    /// do it. A submit that ran before that task was scheduled read the *old*
    /// id and filed the prompt into the previous backend conversation, under a
    /// transcript the UI had already cleared.
    ///
    /// This flag is set before `new_session` returns, so the intent is
    /// observable the moment it is expressed; `session_for` acts on it under
    /// the lock it already takes. An atomic rather than more state in the slot,
    /// because the point is to be readable without awaiting anything.
    reset_session: Arc<std::sync::atomic::AtomicBool>,
    /// Consecutive failed polls, for [`stream_state`](Runtime::stream_state).
    ///
    /// An `AtomicUsize` rather than a lock: it is written from the poll task and
    /// read from the render path, which must never block on the poller.
    poll_failures: Arc<std::sync::atomic::AtomicUsize>,
    /// The hub handle, once the hub has connected.
    ///
    /// The worker surface — the roster, its activity, the watched screens and
    /// every mutation — is the hub's, not the backend's: host-link peers are
    /// this device's business and never reach the orchestration backend. Without it
    /// the Workers tab reads empty and, worse, its mutations inherit the trait's
    /// no-op success.
    hub: HubSlot,
    /// Workspace roots whose `MEDULLA.md` profiles ride every session mint.
    ///
    /// Empty by default: a host that names no workspaces mints plain sessions,
    /// which is what every caller got before this was wired.
    workspaces: Vec<std::path::PathBuf>,
    /// The backend deployment this host is configured for, when one is known.
    ///
    /// The backend configuration this host was built from.
    ///
    /// The whole config, not just its URL. Two surfaces need the credential
    /// half: the feedback board resolves a bearer through the same precedence
    /// chain the runtime's own client used, and `logout` has to know whether an
    /// inline `backend.token` or a non-default `backend.tokenEnv` will keep this
    /// process authenticated after the store is cleared. Reconstructing a
    /// default `BackendConfig` from the URL discarded exactly those two fields,
    /// which made both checks unable to fire.
    ///
    /// `None` — an unconfigured host — leaves the board reporting itself
    /// unavailable rather than dialling a default deployment the operator never
    /// signed in to.
    backend: Option<crate::config::BackendConfig>,
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

impl CloudRuntime {
    /// Wrap a configured backend client.
    ///
    /// Performs no I/O — call [`refresh`](Self::refresh) for that. Construction
    /// and first fetch are separate so a host can paint its UI before the first
    /// round trip lands.
    pub fn new(client: Arc<MedullaClient>) -> Self {
        Self::with_hub(client, HubSlot::default())
    }

    /// Wrap a client and share the slot the hub fills once connected.
    ///
    /// The slot is passed empty at construction and filled later: the hub needs
    /// a signed-in session to connect, which is only established after the
    /// runtime exists.
    pub fn with_hub(client: Arc<MedullaClient>, hub: HubSlot) -> Self {
        Self {
            client,
            cell: Arc::new(SnapshotCell::new()),
            session: SessionSlot::default(),
            cursor: Arc::new(tokio::sync::Mutex::new(None)),
            reset_session: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            poll_failures: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            hub,
            workspaces: Vec::new(),
            backend: None,
        }
    }

    /// Attach the workspace roots whose profiles ride each session mint.
    #[must_use]
    pub fn with_workspaces(mut self, workspaces: Vec<std::path::PathBuf>) -> Self {
        self.workspaces = workspaces;
        self
    }

    /// Record the backend configuration this host was built from.
    ///
    /// The same config the runtime's client came from — kept so the surfaces
    /// that mint their own client resolve a bearer identically, and so `logout`
    /// can see the credential sources that outrank the store.
    #[must_use]
    pub fn with_backend(mut self, backend: crate::config::BackendConfig) -> Self {
        self.backend = Some(backend);
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
    async fn session_for(
        client: &MedullaClient,
        slot: &SessionSlot,
        workspaces: &[std::path::PathBuf],
        reset: &std::sync::atomic::AtomicBool,
    ) -> Result<String, ClientError> {
        let mut guard = slot.lock().await;
        // Honour a `new_session` that has not had its task scheduled yet. Taken
        // under the same lock the mint uses, so two concurrent submits cannot
        // both see the reset and both mint.
        if reset.swap(false, Ordering::AcqRel) {
            *guard = None;
        }
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
        // The `MEDULLA.md` profiles of the configured workspace roots ride the
        // mint (`workspaceProfiles`), which is what lets the backend brief a
        // delegated turn on repositories it has never seen. Roots without a
        // profile are dropped by the collector, so passing every configured one
        // is safe and an empty slice mints a plain session.
        let profiles = crate::init::collect_profile_inputs(workspaces);
        let created = client.create_session_with(None, &profiles).await?;
        tracing::debug!("[cloud_runtime] minted session {}", created.session_id);
        *guard = Some(created.session_id.clone());
        Ok(created.session_id)
    }

    /// Pull the roster and session list, fold them in, and notify.
    ///
    /// Best-effort by design: an unconfigured or signed-out host is an expected
    /// state rather than a failure, so a rejected call leaves the previous
    /// snapshot intact instead of blanking the UI.
    pub async fn refresh(&self) {
        let roster = match self.client.roster().await {
            Ok(workers) => fold::roster(workers),
            Err(err) => {
                tracing::debug!("[cloud_runtime] roster unavailable: {err}");
                Vec::new()
            }
        };
        let threads = match self.client.list_sessions().await {
            Ok(sessions) => fold::threads(sessions),
            Err(err) => {
                tracing::debug!("[cloud_runtime] sessions unavailable: {err}");
                Vec::new()
            }
        };

        tracing::debug!(
            "[cloud_runtime] refresh roster={} threads={}",
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
            tracing::debug!("[cloud_runtime] adopting initial session {id}");
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

        let fetched = match self.client.list_events(&session, after).await {
            Ok(events) => events,
            Err(err) => {
                // Count it rather than only logging. A backend that is
                // unreachable otherwise leaves the transcript inert forever
                // with no signal beyond a debug line nobody is watching.
                let n = self.poll_failures.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::debug!("[cloud_runtime] event poll failed ({n}): {err}");
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
            tracing::debug!("[cloud_runtime] dropping a batch from a superseded session");
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
        tracing::debug!("[cloud_runtime] folded {count} events after {after:?}");
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
                    // Nothing came back. If a turn this host drew locally has
                    // been waiting on the backend since before the deadline,
                    // stop reporting it as running — checked here rather than
                    // on its own timer because this loop already wakes at least
                    // once per `POLL_IDLE`, which is finer than the deadline.
                    if rt.cell.expire_stalled_echo(ECHO_STALL_AFTER) {
                        tracing::debug!(
                            "[cloud_runtime] no events {}s after a submitted turn; \
                             settling the running indicator",
                            ECHO_STALL_AFTER.as_secs()
                        );
                    }
                    // Ease back rather than snapping to idle: a turn often has
                    // gaps between batches, and snapping would add the full
                    // idle delay to the very next token.
                    (delay * 2).min(POLL_IDLE)
                };
            }
        })
    }

    /// The backend client, for callers needing something the trait does not
    /// model.
    pub fn client(&self) -> &Arc<MedullaClient> {
        &self.client
    }
}

impl Runtime for CloudRuntime {
    fn describe(&self) -> String {
        "medulla (cloud backend)".to_string()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        self.cell.snapshot()
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.cell.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let client = Arc::clone(&self.client);
        let session = self.session_handle();
        let workspaces = self.workspaces.clone();
        let reset = Arc::clone(&self.reset_session);
        let cell = Arc::clone(&self.cell);
        // Draw the turn before anything is dialled. Everything below this line
        // is at least one round trip away — on the first turn of a session, two
        // — and the replay that would otherwise be the turn's first appearance
        // is a further poll interval behind that.
        let echo = cell.echo_user(&input);
        Box::pin(async move {
            // Non-blocking: the reply arrives over the event stream, so the UI
            // can render progress instead of freezing until the turn finishes.
            let sent: anyhow::Result<()> = async {
                let id = Self::session_for(&client, &session, &workspaces, &reset).await?;
                client.send_message(&id, &input, false).await?;
                Ok(())
            }
            .await;
            if sent.is_err() {
                // Nothing accepted the turn, so no confirmation is coming and
                // the spinner would turn forever waiting for one.
                cell.abandon_echo(echo);
            }
            sent
        })
    }

    fn steering_reaches_backend(&self) -> bool {
        // The backend models a session abort but has no route for answering a
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
        let client = Arc::clone(&self.client);
        let session = self.session_handle();
        // Fire-and-forget: the trait is sync, and an abort that has not landed
        // yet is still better than blocking the UI thread on a round trip.
        tokio::spawn(async move {
            let Some(id) = session.lock().await.clone() else {
                tracing::debug!("[cloud_runtime] abort with no active session");
                return;
            };
            if let Err(err) = client.abort(&id).await {
                tracing::debug!("[cloud_runtime] abort failed: {err}");
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

    fn cancel_task(&self, cycle_id: String, task_id: String) {
        // Control-plane activity uses its server-minted abort handle as the
        // task half of the row key. It never went through the backend steering
        // API, so cancel it on the same local runner that admitted it.
        if cycle_id.starts_with("mcp:") {
            if let Some(hub) = self.hub() {
                hub.task_runner().abort_task(&task_id);
            }
        }
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

    fn hand_off_harness(
        &self,
        brief: crate::hub::HarnessHandoff,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let hub = self.hub();
        Box::pin(async move {
            let Some(hub) = hub else {
                return Err(anyhow::anyhow!(
                    "no hub is connected, so the orchestrator cannot be sent your brief"
                ));
            };
            hub.hand_off_harness(brief).await
        })
    }

    fn hold_harness(
        &self,
        workspace: String,
        reason: Option<String>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let hub = self.hub();
        Box::pin(async move {
            let Some(hub) = hub else {
                return Err(anyhow::anyhow!(
                    "no hub is connected, so the orchestrator cannot be told you took this harness"
                ));
            };
            hub.hold_harness(&workspace, reason, crate::clock::now_millis())
                .await
        })
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

    fn kill_task(&self, worker: String, task_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let hub = self.hub();
        Box::pin(async move {
            let Some(hub) = hub else {
                return Ok(());
            };
            hub.kill(&worker, &task_id)
                .await
                .map_err(|e| anyhow::anyhow!(e))
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
        let backend = self.backend.clone();
        // Removing the store's file is the whole logout as far as *this* host's
        // own credential goes — there is no keychain entry or profile beside it
        // that a next start could read back.
        //
        // It is not the whole story for the process, though: `backend.token` and
        // `backend.tokenEnv` outrank the store in
        // [`crate::auth::resolve_backend_token`], so an operator running on an
        // inline token or a custom variable stays authenticated after a
        // "successful" logout. That is worth an error rather than a silent
        // half-measure — telling someone they signed out while they are still
        // signed in is the failure mode logging out exists to prevent.
        Box::pin(async move {
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            crate::auth::clear(&env)
                .map_err(|e| anyhow::anyhow!("the stored session could not be removed: {e}"))?;
            // Resolved against the *real* config, with `session` deliberately
            // `None`: the store has just been cleared, so anything that still
            // resolves is by definition a source that outranks it.
            if let Some(backend) = backend.as_ref() {
                if crate::auth::resolve_backend_token(&env, backend, None).is_some() {
                    let source = if backend.token.is_some() {
                        "`backend.token` in the config".to_string()
                    } else {
                        format!("the `{}` environment variable", backend.token_env)
                    };
                    anyhow::bail!(
                        "the stored session was cleared, but this process is still \
                         authenticated from {source} — remove it to finish signing out"
                    );
                }
            }
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
        // Announced synchronously, before this returns: a submit racing the
        // spawn below would otherwise read the previous id and file the prompt
        // into the conversation the operator just left.
        self.reset_session
            .store(true, std::sync::atomic::Ordering::Release);
        // Clear rather than mint: the next submit mints one. Minting here would
        // create a durable session the operator may never use. The task still
        // runs, for the cursor and to settle the slot even if no submit follows.
        tokio::spawn(async move {
            *session.lock().await = None;
            *cursor.lock().await = None;
            tracing::debug!("[cloud_runtime] active session cleared");
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
            tracing::debug!("[cloud_runtime] active session switched to {id}");
        });
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        // The chat-tree store has no backend route yet. Empty rather than an
        // error: "no saved chats" is a state the Chat tab already renders,
        // whereas an error would show a failure where there is none.
        Box::pin(async { Ok(Vec::new()) })
    }

    fn resume_chat(&self, _main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Err(anyhow::anyhow!(NOT_YET_WIRED)) })
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        // Nothing process-scoped to tear down: the client is a pooled HTTP
        // handle and the poll task ends with the runtime. Kept as an explicit
        // no-op rather than removed so the trait's shutdown contract stays
        // visible at this impl.
        Box::pin(async { Ok(()) })
    }

    // --- feedback board ---------------------------------------------------
    // A different route family from the orchestration API; the bodies live in
    // [`feedback`], which mints a client per call from the stored session.

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
