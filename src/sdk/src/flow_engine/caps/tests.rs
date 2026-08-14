//! Unit tests for [`Assembly`], focused on the deadline it hands a spawned
//! task.
//!
//! A `spawn` node's task runs detached from the super-step that started it, so
//! [`TaskCapabilities::timeout`] is the only thing standing between a wedged
//! task and one that outlives the run itself. These tests exercise it directly
//! rather than through a whole run, because the bug it guards against —
//! `Assembly::clone` handing every task a fresh `run_timeout_secs` window
//! instead of what is left of the run's own deadline — is invisible from a
//! passing end-to-end test that never spawns two tasks far enough apart in
//! time to tell the difference.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tinyflows::caps::mock::MockWorkflowResolver;

use super::Assembly;
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::flow_engine::caps::tasks::TaskCapabilities;
use crate::flow_engine::settings::CapabilitySettings;
use crate::hub::{RunError, TaskOutcome, TaskRequest};

/// A dispatch these tests never reach: they only ever call `timeout()`.
struct UnusedDispatch;

#[async_trait]
impl HarnessDispatch for UnusedDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        unreachable!("these tests never dispatch a harness task")
    }
}

/// An `Assembly` with `run_timeout_secs` as given, and a deadline computed the
/// same way [`build_capabilities_inner`](super::build_capabilities_inner) does.
fn assembly(run_timeout_secs: u64) -> Assembly {
    let mut settings =
        CapabilitySettings::rooted_at(std::env::temp_dir().join("medulla-caps-mod-tests"));
    settings.run_timeout_secs = run_timeout_secs;
    Assembly {
        settings: Arc::new(settings),
        dispatch: Arc::new(UnusedDispatch),
        resolver: Arc::new(MockWorkflowResolver::default()),
        http_credentials: HashMap::new(),
        node_progress: None,
        state_namespace: "test".to_string(),
        run_id: "test-run".to_string(),
        evidence: None,
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
        sequence: Arc::new(AtomicU64::new(0)),
        deadline: Instant::now() + Duration::from_secs(run_timeout_secs.max(1)),
        depth: 0,
    }
}

/// The core regression: a task's remaining budget must track the run's fixed
/// deadline, not restart from `run_timeout_secs` on every read. Before the fix,
/// `timeout()` recomputed `Duration::from_secs(run_timeout_secs)` on every
/// call, so this would have been `first == second` regardless of the sleep.
#[test]
fn a_spawned_task_s_deadline_shrinks_toward_the_run_s_own_rather_than_resetting() {
    let bundle = assembly(1000);
    let first = bundle.timeout();
    std::thread::sleep(Duration::from_millis(50));
    let second = bundle.timeout();
    assert!(
        second < first,
        "a task's remaining deadline must shrink as the run's own deadline \
         approaches, not stay pinned to a fresh run_timeout_secs window: \
         first={first:?} second={second:?}"
    );
    assert!(
        first - second < Duration::from_secs(1),
        "the drop should track the elapsed wall-clock time, not jump by a \
         whole run_timeout_secs: first={first:?} second={second:?}"
    );
}

/// A misconfigured `run_timeout_secs: 0` reaching the assembly directly (rather
/// than through `CapabilitySettings::from_config`, which already floors it)
/// must not hand every spawned task an already-expired deadline — that fails
/// every task started under it, immediately and silently.
#[test]
fn a_zero_run_timeout_still_gives_a_spawned_task_a_moment_to_run() {
    let bundle = assembly(0);
    assert!(
        bundle.timeout() > Duration::ZERO,
        "a zero run_timeout_secs must not produce an already-expired deadline"
    );
}
