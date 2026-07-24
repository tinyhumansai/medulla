//! [`SessionManager`] — the operator-facing surface the Sessions tab drives, and
//! the daemon-facing entry that runs a folded [`TurnRequest`].
//!
//! It owns the live sessions (processes for the interactive transport, records
//! for everything else), delegates continuity decisions to
//! [`SessionRegistry`](super::registry::SessionRegistry), and publishes a change
//! ping so the UI can redraw without polling.
//!
//! - [`types`] — configuration, the open request, and the transcript model.
//! - [`turns`] — turn execution: the bounded/unbound split and the interactive
//!   and one-shot transports.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::daemon::providers::{Abort, RunTaskFn};
use crate::tinyplace::HarnessProvider;

use super::input::Observation;
use super::registry::SessionRegistry;
use super::routing::{route_transport, Transport};
use super::types::{SessionClass, SessionKey, SessionPhase, SessionRecord, TurnRequest};

mod turns;
pub mod types;

pub use types::{OpenSession, SessionConfig, TranscriptLine, TranscriptRole};

use types::{SessionEntry, TRANSCRIPT_CAP};

/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

pub(super) struct Inner {
    pub(super) config: SessionConfig,
    pub(super) registry: SessionRegistry,
    pub(super) run_task: RunTaskFn,
    pub(super) now: NowFn,
    pub(super) sessions: Mutex<Vec<SessionEntry>>,
    pub(super) next_id: AtomicU64,
    pub(super) changed: broadcast::Sender<()>,
}

/// Spins up and manages coding-agent sessions in both lifetime classes.
///
/// Cheap to clone (an `Arc`), so the daemon, the hub, and the TUI can share one.
#[derive(Clone)]
pub struct SessionManager {
    pub(super) inner: Arc<Inner>,
}

impl SessionManager {
    /// Build a manager over `config`, using `run_task` for the one-shot
    /// transport.
    pub fn new(config: SessionConfig, run_task: RunTaskFn) -> Self {
        let (changed, _) = broadcast::channel(64);
        SessionManager {
            inner: Arc::new(Inner {
                config,
                registry: SessionRegistry::default(),
                run_task,
                now: Arc::new(crate::clock::now_millis),
                sessions: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                changed,
            }),
        }
    }

    /// Override the clock (tests).
    ///
    /// Must be called before the manager is cloned or any session is opened.
    pub fn with_now(self, now: NowFn) -> Self {
        let inner = Arc::try_unwrap(self.inner)
            .unwrap_or_else(|_| panic!("with_now must be called before cloning/opening"));
        SessionManager {
            inner: Arc::new(Inner { now, ..inner }),
        }
    }

    /// The shared binding registry, for a daemon that wants to reset a peer's
    /// conversation directly.
    pub fn registry(&self) -> &SessionRegistry {
        &self.inner.registry
    }

    /// A change notification channel — a ping fires after every mutation.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.inner.changed.subscribe()
    }

    /// The current clock reading, in epoch ms.
    pub(super) fn now(&self) -> i64 {
        (self.inner.now)()
    }

    /// Publish a change ping. A send failure only means nobody is listening.
    pub(super) fn notify(&self) {
        let _ = self.inner.changed.send(());
    }

    /// Every session, oldest first — the Sessions tab's list.
    pub fn records(&self) -> Vec<SessionRecord> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry.record.clone())
            .collect()
    }

    /// One session's record by id.
    pub fn record(&self, id: &str) -> Option<SessionRecord> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.record.id == id)
            .map(|entry| entry.record.clone())
    }

    /// One session's transcript, oldest line first.
    pub fn transcript(&self, id: &str) -> Vec<TranscriptLine> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.record.id == id)
            .map(|entry| entry.transcript.clone())
            .unwrap_or_default()
    }

    /// Register a new session and return its id.
    ///
    /// Registration does **not** start a process: an unbound interactive session
    /// stays [`SessionPhase::Idle`] until its first turn, so an opened-but-unused
    /// session costs nothing. Re-opening an existing conversation on the same
    /// provider returns the existing id rather than creating a rival session.
    pub fn open(&self, request: OpenSession) -> String {
        let provider = request
            .provider
            .unwrap_or(self.inner.config.default_provider);
        let key = SessionKey::new(request.conversation, provider);
        let class = request.class.unwrap_or(SessionClass::Unbound);

        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(existing) = sessions
            .iter()
            .find(|entry| entry.record.key == key && !entry.record.phase.is_terminal())
        {
            return existing.record.id.clone();
        }
        let now = (self.inner.now)();
        let id = format!("s_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        sessions.push(SessionEntry {
            record: SessionRecord {
                id: id.clone(),
                key,
                class,
                driver: request.driver,
                phase: SessionPhase::Idle,
                workspace: request
                    .workspace
                    .unwrap_or_else(|| self.inner.config.workspace.clone()),
                harness_session_id: None,
                turns: 0,
                created_at: now,
                last_at: now,
                last_error: None,
            },
            live: None,
            abort: Abort::new(),
            transcript: Vec::new(),
            model: request.model,
        });
        drop(sessions);
        self.notify();
        id
    }

    /// How a session's child process is driven, given its class and provider.
    pub fn transport(&self, id: &str) -> Option<Transport> {
        self.record(id)
            .map(|record| route_transport(record.class, record.key.provider))
    }

    /// Close a session: interrupt any in-flight turn, tear down the process, and
    /// mark the record terminal.
    ///
    /// The record is retained (as [`SessionPhase::Closed`]) so the operator can
    /// still read its transcript; [`SessionManager::forget`] drops it entirely.
    pub async fn close(&self, id: &str) {
        let live = {
            let mut sessions = self.inner.sessions.lock().unwrap();
            let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) else {
                return;
            };
            entry.abort.abort();
            entry.record.phase = SessionPhase::Closed;
            entry.record.last_at = (self.inner.now)();
            entry.live.take()
        };
        if let Some(live) = live {
            live.close().await;
        }
        if let Some(record) = self.record(id) {
            self.inner.registry.prune_chain(&record.key);
        }
        self.push_line_by_id(id, TranscriptRole::Status, "session closed");
        self.notify();
    }

    /// Drop a closed session's record and transcript.
    ///
    /// Refuses to drop a session that is still live — close it first, so a
    /// forgotten session can never leave an orphaned process behind.
    pub fn forget(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(index) = sessions.iter().position(|entry| {
            entry.record.id == id && entry.record.phase.is_terminal() && entry.live.is_none()
        }) else {
            return false;
        };
        sessions.remove(index);
        drop(sessions);
        self.notify();
        true
    }

    /// Interrupt the turn in flight on `id`, leaving the session alive.
    ///
    /// Returns whether a turn was actually running. An interrupt ends the
    /// **turn**, never the session: the next turn on an unbound session still
    /// carries the conversation.
    pub fn interrupt(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) else {
            return false;
        };
        if entry.record.phase != SessionPhase::Turn {
            return false;
        }
        entry.abort.abort();
        entry.record.phase = SessionPhase::Interrupting;
        entry.record.last_at = (self.inner.now)();
        drop(sessions);
        self.notify();
        true
    }

    /// Drop the conversation's binding so its next turn starts with no context.
    ///
    /// This is what "reset"/"clear" means: the binding *is* the conversation. A
    /// literal `/clear` prompt is never sent — `codex exec` has no slash
    /// commands and would read it as task text.
    pub fn reset(&self, id: &str) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        let dropped = self.inner.registry.reset(&record.key);
        self.push_line_by_id(
            id,
            TranscriptRole::Status,
            if dropped {
                "context reset — the next turn starts fresh"
            } else {
                "no bound context to reset"
            },
        );
        self.notify();
        dropped
    }

    /// Fold an envelope-driven [`Observation`] into a session record.
    ///
    /// Creates the session on first sight so an observation whose opening event
    /// was missed still produces a row. Never spawns a process: an
    /// envelope-driven session is run by a remote wrapper and only *observed*
    /// here.
    pub fn observe(&self, observation: &Observation) {
        let now = (self.inner.now)();
        let mut sessions = self.inner.sessions.lock().unwrap();
        let index = match sessions
            .iter()
            .position(|entry| entry.record.key == observation.key)
        {
            Some(index) => index,
            None => {
                let id = format!("s_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
                sessions.push(SessionEntry {
                    record: SessionRecord {
                        id,
                        key: observation.key.clone(),
                        // An observed session is by definition long-lived: the
                        // wrapper keeps it across turns.
                        class: SessionClass::Unbound,
                        driver: super::types::SessionDriver::Envelope,
                        phase: SessionPhase::Live,
                        workspace: observation.cwd.clone(),
                        harness_session_id: None,
                        turns: 0,
                        created_at: now,
                        last_at: now,
                        last_error: None,
                    },
                    live: None,
                    abort: Abort::new(),
                    transcript: Vec::new(),
                    model: None,
                });
                sessions.len() - 1
            }
        };
        let entry = &mut sessions[index];
        entry.record.last_at = now;
        if !observation.harness_session_id.is_empty() {
            entry.record.harness_session_id = Some(observation.harness_session_id.clone());
        }
        if observation.ends_turn {
            entry.record.turns += 1;
        }
        if observation.is_error {
            entry.record.last_error = Some(observation.detail.clone());
        }
        let role = if observation.is_error {
            TranscriptRole::Error
        } else {
            TranscriptRole::Status
        };
        push_line(&mut entry.transcript, now, role, &observation.detail);
        drop(sessions);
        self.notify();
    }

    /// Append a transcript line to the session with `id`, if it still exists.
    pub(super) fn push_line_by_id(&self, id: &str, role: TranscriptRole, text: &str) {
        let now = (self.inner.now)();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
            push_line(&mut entry.transcript, now, role, text);
        }
    }

    /// The provider a turn request resolves to.
    pub(super) fn provider_for(&self, request: &TurnRequest) -> HarnessProvider {
        request.key.provider
    }
}

/// Append `text` to a transcript, dropping the oldest line past the cap.
pub(super) fn push_line(
    transcript: &mut Vec<TranscriptLine>,
    at: i64,
    role: TranscriptRole,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    transcript.push(TranscriptLine {
        at,
        role,
        text: text.trim().to_string(),
    });
    if transcript.len() > TRANSCRIPT_CAP {
        let overflow = transcript.len() - TRANSCRIPT_CAP;
        transcript.drain(0..overflow);
    }
}
