//! What happens when the *terminal* write of a run record fails.
//!
//! The run body and the [`RunFinalizer`](super::super::super::RunFinalizer)
//! drop guard are the only two writers of a run's final status, and exactly one
//! of them must do it. The ordering that makes that true is "write, then
//! disarm": disarming first would defuse the guard on the strength of a write
//! that had not happened yet, and a store that then refused the write would
//! leave the run at `Running` with nobody left to correct it.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::workflows::types::{
    RunRecord, WorkflowNote, WorkflowProposal, WorkflowRecord, WorkflowRevision, WorkflowSummary,
};

/// A store that delegates everything but refuses the `nth` `record_run` call.
///
/// Refusing by call number rather than by content is what lets a test fail the
/// terminal write while still allowing the drop guard's reconciling write
/// through — which is the behaviour under test.
struct FailNthRecordRun {
    inner: Arc<FileWorkflowStore>,
    nth: usize,
    calls: AtomicUsize,
}

impl FailNthRecordRun {
    fn new(inner: Arc<FileWorkflowStore>, nth: usize) -> Self {
        Self {
            inner,
            nth,
            calls: AtomicUsize::new(0),
        }
    }
}

impl WorkflowStore for FailNthRecordRun {
    fn list(&self) -> Result<Vec<WorkflowSummary>, WorkflowError> {
        self.inner.list()
    }

    fn get(&self, id: &str) -> Result<Option<WorkflowRecord>, WorkflowError> {
        self.inner.get(id)
    }

    fn save(&self, record: &WorkflowRecord) -> Result<(), WorkflowError> {
        self.inner.save(record)
    }

    fn delete(&self, id: &str) -> Result<(), WorkflowError> {
        self.inner.delete(id)
    }

    fn record_run(&self, run: &RunRecord) -> Result<(), WorkflowError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.nth {
            return Err(WorkflowError::Engine("disk is full".to_string()));
        }
        self.inner.record_run(run)
    }

    fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, WorkflowError> {
        self.inner.get_run(run_id)
    }

    fn list_runs(&self, workflow_id: &str) -> Result<Vec<RunRecord>, WorkflowError> {
        self.inner.list_runs(workflow_id)
    }

    fn list_revisions(&self, workflow_id: &str) -> Result<Vec<WorkflowRevision>, WorkflowError> {
        self.inner.list_revisions(workflow_id)
    }

    fn revision(
        &self,
        workflow_id: &str,
        revision_id: &str,
    ) -> Result<Option<WorkflowRevision>, WorkflowError> {
        self.inner.revision(workflow_id, revision_id)
    }

    fn list_notes(&self, workflow_id: &str) -> Result<Vec<WorkflowNote>, WorkflowError> {
        self.inner.list_notes(workflow_id)
    }

    fn append_note(&self, note: &WorkflowNote) -> Result<(), WorkflowError> {
        self.inner.append_note(note)
    }

    fn save_proposal(&self, proposal: &WorkflowProposal) -> Result<(), WorkflowError> {
        self.inner.save_proposal(proposal)
    }
}

/// Build a run context over `store`, with the harness's own settings.
fn context_over(harness: &Harness, store: Arc<dyn WorkflowStore>) -> RunContext {
    let mut context = harness.context(Arc::new(StubDispatch::default()));
    context.services.resolver = Arc::new(StoreWorkflowResolver::new(
        store.clone(),
        harness.settings.max_loop_iterations,
    ));
    context.store = store;
    context
}

#[tokio::test]
async fn a_failed_terminal_write_still_leaves_the_run_reconciled() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    // Call 1 is the opening `Running` record; call 2 is the terminal one.
    let store = Arc::new(FailNthRecordRun::new(harness.store.clone(), 2));

    let error = run_workflow(
        context_over(&harness, store.clone()),
        "diamond",
        "run-terminal-write",
        json!({}),
        Default::default(),
    )
    .await
    .expect_err("a refused terminal write must surface");
    assert!(error.to_string().contains("disk is full"), "{error}");

    // The guard was still armed, so it reconciled the record rather than
    // leaving it claiming to be running.
    let record = require_run(harness.store.as_ref(), "run-terminal-write").expect("recorded");
    assert_eq!(record.status, RunStatus::Interrupted);
    assert!(record.finished_at.is_some());
}

#[tokio::test]
async fn a_failed_terminal_write_on_resume_still_leaves_the_run_reconciled() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let store = Arc::new(FailNthRecordRun::new(harness.store.clone(), 4));

    let paused = run_workflow(
        context_over(&harness, store.clone()),
        "gated",
        "run-resume-write",
        json!({}),
        Default::default(),
    )
    .await
    .expect("pauses on its gate");
    assert_eq!(paused.status, RunStatus::PendingApproval);

    // Calls so far: the opening record and the pending-approval one. The resume
    // leg writes `Running` (3) and then its terminal record (4), which fails.
    let error = resume_workflow(
        context_over(&harness, store.clone()),
        "run-resume-write",
        vec!["review".to_string()],
        Vec::new(),
    )
    .await
    .expect_err("a refused terminal write must surface");
    assert!(error.to_string().contains("disk is full"), "{error}");

    let record = require_run(harness.store.as_ref(), "run-resume-write").expect("recorded");
    assert_eq!(record.status, RunStatus::Interrupted);
}
