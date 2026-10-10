//! Lazy daemon ownership of the single process runtime. The registry keeps only
//! a weak reference, so the last host releases the runtime and its scoped seams.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as RegistryMutex, OnceLock, Weak};

use openhuman_embed::{Runtime, RuntimeConfig, TokenSource, Workspace};
use tokio::sync::{Mutex, OnceCell};

#[derive(Default)]
pub struct EmbedHost {
    runtime: OnceCell<Arc<OwnedRuntime>>,
    remaining: Option<AtomicU64>,
}

pub(super) struct OwnedRuntime {
    pub runtime: Runtime,
    home: PathBuf,
    pub turns: ThreadClaims,
}

impl EmbedHost {
    /// Bind the daemon’s configured provider budgets to its native runtime.
    pub fn with_budget(budget: Option<crate::config::BudgetConfig>) -> Self {
        let headroom = budget
            .as_ref()
            .and_then(|config| config.for_provider("openhuman"))
            .and_then(|budget| {
                budget.remaining_tokens.or_else(|| {
                    budget
                        .limit_tokens
                        .map(|limit| limit.saturating_sub(budget.used_tokens.unwrap_or(0)))
                })
            })
            .map(|remaining| remaining.max(0) as u64);
        Self {
            runtime: OnceCell::new(),
            remaining: headroom.map(AtomicU64::new),
        }
    }
    pub(super) fn token_headroom(&self) -> Option<u64> {
        self.remaining
            .as_ref()
            .map(|remaining| remaining.load(Ordering::SeqCst))
    }
    pub(super) fn charge(&self, tokens: u64) {
        if let Some(remaining) = &self.remaining {
            let _ = remaining.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                Some(value.saturating_sub(tokens))
            });
        }
    }

    pub(super) async fn runtime(&self, home: &Path) -> Result<Arc<OwnedRuntime>, String> {
        static PROCESS: OnceLock<Mutex<Weak<OwnedRuntime>>> = OnceLock::new();
        let owned = self
            .runtime
            .get_or_try_init(|| async {
                let mut registry = PROCESS.get_or_init(|| Mutex::new(Weak::new())).lock().await;
                if let Some(runtime) = registry.upgrade() {
                    if runtime.home != home {
                        return Err(
                            "the process already hosts another Medulla account's embed runtime"
                                .into(),
                        );
                    }
                    return Ok(runtime);
                }
                let mut config = RuntimeConfig::default();
                config.local_ai.runtime_enabled = false;
                config.runtime_python.enabled = false;
                config.memory.conversations.enabled = false;
                config.agent.session_dual_write = false;
                config.agent.session_shadow_reads = false;
                let runtime = Runtime::builder()
                    .config(config)
                    .workspace(Workspace::Dir(home.join("embed").join("workspace")))
                    .token(TokenSource::Fixed(uuid::Uuid::new_v4().to_string().into()))
                    .build()
                    .await
                    .map_err(|error| format!("embed runtime: {error}"))?;
                let owned = Arc::new(OwnedRuntime {
                    runtime,
                    home: home.to_owned(),
                    turns: ThreadClaims::default(),
                });
                *registry = Arc::downgrade(&owned);
                Ok::<_, String>(owned)
            })
            .await?;
        if owned.home != home {
            return Err("a daemon cannot change its embed account home between turns".into());
        }
        Ok(owned.clone())
    }
}

/// A resumed thread has one owner while its policy snapshot is replaced.
#[derive(Default)]
pub(super) struct ThreadClaims(RegistryMutex<HashMap<String, Weak<Mutex<()>>>>);

impl ThreadClaims {
    pub fn claim(&self, thread: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        let lock = {
            let mut locks = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(thread).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(thread.to_owned(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.try_lock_owned()
            .map_err(|_| "the native session already has an active turn".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_session_policy_snapshot_has_one_owner_without_blocking_other_workers() {
        let claims = ThreadClaims::default();
        let first = claims.claim("a").unwrap();
        assert!(claims.claim("a").is_err());
        let other = claims.claim("b").unwrap();
        drop(first);
        assert!(claims.claim("a").is_ok());
        drop(other);
    }
}
