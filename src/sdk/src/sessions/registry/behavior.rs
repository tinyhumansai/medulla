//! Binding lifecycle and per-conversation turn serialization.
//!
//! The registry stores *bindings*, not processes. A binding is the harness's own
//! session id captured from a completed turn, remembered so the next turn on
//! that conversation can resume it. Processes live in
//! [`SessionManager`](crate::sessions::manager::SessionManager).
//!
//! Two invariants this module exists to hold:
//!
//! - **Capture, never preset.** `claude --session-id <uuid>` can preset an id,
//!   but a second start with the same id errors `Session ID … is already in
//!   use`. Capturing the id the CLI announces is symmetric with codex's
//!   `thread_id` and has no reuse hazard.
//! - **Reset drops the binding; it never prompts.** Sending a literal `/clear`
//!   would be read as task text by `codex exec`, which has no slash commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use super::types::{Inner, SessionBinding, SessionRegistry, TurnPlan, WorkspaceContext};
use crate::sessions::routing::can_resume;
use crate::sessions::types::{SessionClass, SessionKey};

/// How many conversation bindings to remember before evicting the least recently
/// used. Bindings are cheap (two short strings) but unbounded growth over a
/// long-lived daemon is not.
pub const DEFAULT_MAX_BINDINGS: usize = 256;

impl TurnPlan {
    /// A plan for a turn that neither resumes nor binds.
    fn stateless(class: SessionClass) -> Self {
        TurnPlan {
            class,
            resume_session_id: None,
            workspace_context: WorkspaceContext::default(),
            bind: false,
        }
    }
}

impl Inner {
    fn get(&self, key: &str) -> Option<&SessionBinding> {
        self.bindings.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Insert or refresh a binding, moving it to the most-recent end and
    /// evicting from the front while over `max`.
    fn record(&mut self, key: String, binding: SessionBinding, max: usize) {
        self.bindings.retain(|(k, _)| k != &key);
        self.bindings.push((key, binding));
        while self.bindings.len() > max.max(1) {
            self.bindings.remove(0);
        }
    }

    fn forget(&mut self, key: &str) -> bool {
        let before = self.bindings.len();
        self.bindings.retain(|(k, _)| k != key);
        before != self.bindings.len()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BINDINGS)
    }
}

impl SessionRegistry {
    /// Build a registry remembering at most `max_bindings` conversations.
    pub fn new(max_bindings: usize) -> Self {
        SessionRegistry {
            inner: Arc::new(Mutex::new(Inner::default())),
            chains: Arc::new(Mutex::new(HashMap::new())),
            max_bindings: max_bindings.max(1),
        }
    }

    /// Decide what one turn should do about continuity.
    ///
    /// A [`SessionClass::Bounded`] turn returns immediately with nothing to
    /// resume and nothing to bind — a one-shot run is already context-free, and
    /// touching the map would leak task context into a conversation.
    pub fn plan(&self, key: &SessionKey, class: SessionClass) -> TurnPlan {
        if class == SessionClass::Bounded {
            return TurnPlan::stateless(class);
        }
        if !can_resume(key.provider) {
            // No resume flag on this CLI: every turn runs fresh. Recording a
            // binding we can never act on would only mislead the UI.
            return TurnPlan::stateless(class);
        }
        let map_key = key.map_key();
        let existing = self.inner.lock().unwrap().get(&map_key).cloned();
        match existing {
            Some(binding) => TurnPlan {
                class,
                resume_session_id: Some(binding.session_id),
                workspace_context: binding.workspace_context,
                bind: false,
            },
            None => TurnPlan {
                class,
                resume_session_id: None,
                workspace_context: WorkspaceContext::default(),
                bind: true,
            },
        }
    }

    /// Remember `session_id` as `key`'s binding, refreshing its recency.
    pub fn record(&self, key: &SessionKey, session_id: impl Into<String>) {
        self.record_with_workspace_context(key, session_id, WorkspaceContext::default());
    }

    /// Remember a session id and its mapper workspace state atomically.
    pub fn record_with_workspace_context(
        &self,
        key: &SessionKey,
        session_id: impl Into<String>,
        workspace_context: WorkspaceContext,
    ) {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return;
        }
        self.inner.lock().unwrap().record(
            key.map_key(),
            SessionBinding {
                session_id,
                workspace_context,
            },
            self.max_bindings,
        );
    }

    /// Replace the workspace state attached to an existing binding.
    pub fn record_workspace_context(&self, key: &SessionKey, context: WorkspaceContext) {
        if let Some(binding) = self
            .inner
            .lock()
            .unwrap()
            .bindings
            .iter_mut()
            .find_map(|(map_key, binding)| (map_key == &key.map_key()).then_some(binding))
        {
            binding.workspace_context = context;
        }
    }

    /// The harness session id currently bound to `key`, if any.
    pub fn bound(&self, key: &SessionKey) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .get(&key.map_key())
            .map(|binding| binding.session_id.clone())
    }

    /// Drop `key`'s binding so the next turn starts a fresh session. This is
    /// what "reset"/"clear" means here — the binding is the conversation.
    ///
    /// Returns whether a binding was actually dropped.
    pub fn reset(&self, key: &SessionKey) -> bool {
        self.inner.lock().unwrap().forget(&key.map_key())
    }

    /// Drop every binding for `conversation` across all providers.
    ///
    /// Matching is exact on the provider-prefixed key's conversation half, never
    /// a substring scan: resetting `bob` must not wipe `alicebob`.
    pub fn reset_conversation(&self, conversation: &str) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.bindings.len();
        inner.bindings.retain(|(k, _)| {
            // `map_key` is "<provider> <conversation>"; split once and compare
            // the conversation half exactly.
            k.split_once(' ').map(|(_, c)| c) != Some(conversation)
        });
        before - inner.bindings.len()
    }

    /// How many conversations currently hold a binding.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().bindings.len()
    }

    /// Whether no conversation holds a binding.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Acquire the turn chain for `key` when `class` requires serialization.
    ///
    /// Returns a guard the caller holds for the duration of the turn, or `None`
    /// for a [`SessionClass::Bounded`] turn, which must **not** serialize —
    /// queueing independent task work behind an unrelated conversation would
    /// convert a concurrency budget into a single file.
    ///
    /// The chain is keyed per conversation, so two different peers never wait on
    /// each other.
    pub async fn acquire_turn(
        &self,
        key: &SessionKey,
        class: SessionClass,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        if !class.serializes() {
            return None;
        }
        let lock = {
            let mut chains = self.chains.lock().unwrap();
            chains
                .entry(key.map_key())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        Some(lock.lock_owned().await)
    }

    /// Drop the turn chain for `key` when nobody is waiting on it.
    ///
    /// Called after a session closes. Pruning while a turn is queued would drop
    /// a chain a later turn is still waiting on, so this only removes a chain
    /// whose `Arc` the registry alone holds.
    pub fn prune_chain(&self, key: &SessionKey) {
        let mut chains = self.chains.lock().unwrap();
        let map_key = key.map_key();
        let drop_it = chains
            .get(&map_key)
            .map(|lock| Arc::strong_count(lock) == 1)
            .unwrap_or(false);
        if drop_it {
            chains.remove(&map_key);
        }
    }
}
