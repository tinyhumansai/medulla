//! Shared fixtures for the copilot turn tests.
//!
//! The store is real (a temporary directory) and the diff is real; only the
//! harness is a stand-in, because the alternative is starting a coding agent.
//! The stand-in is where the *edits* come from too: a real copilot edits through
//! the MCP tools, and a stub that writes to the same store is indistinguishable
//! from that as far as [`super::tests`] and [`super::create_tests`] are
//! concerned. Split into its own module so both test files can share one copy
//! of these fixtures rather than each growing past the 500-line file ceiling
//! keeping its own.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::hub::{RunError, TaskOutcome};
use crate::workflows::{FileWorkflowStore, WorkflowRecord};

/// A workflow document with one trigger and one agent step.
pub(super) fn document(id: &str) -> WorkflowRecord {
    WorkflowRecord {
        id: id.to_string(),
        name: "Nightly sweep".into(),
        description: "sweeps".into(),
        enabled: true,
        defaults: Default::default(),
        graph: serde_json::from_value(json!({
            "name": "Nightly sweep",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "do it" } },
            ],
            "edges": [{ "from_node": "start", "to_node": "work" }],
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

/// What a stand-in harness does to the store before it replies — standing in
/// for the tool calls a real copilot turn would have made.
type StoreEdit = Box<dyn Fn(&dyn WorkflowStore) + Send + Sync>;

/// A harness stand-in: replies with `reply`, optionally editing the store first
/// (which is what a real copilot's tool calls would have done), and reports
/// `statuses` as progress along the way.
pub(super) struct StubHarness {
    pub(super) reply: String,
    pub(super) statuses: Vec<String>,
    pub(super) edit: Option<StoreEdit>,
    pub(super) seen: Mutex<Vec<TaskRequest>>,
    store: Arc<dyn WorkflowStore>,
}

impl StubHarness {
    pub(super) fn new(store: Arc<dyn WorkflowStore>, reply: &str) -> Self {
        Self {
            reply: reply.to_string(),
            statuses: Vec::new(),
            edit: None,
            seen: Mutex::new(Vec::new()),
            store,
        }
    }
}

#[async_trait]
impl HarnessDispatch for StubHarness {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        if let Some(edit) = &self.edit {
            edit(self.store.as_ref());
        }
        Ok(TaskOutcome {
            reply: self.reply.clone(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
            transcript: Vec::new(),
        })
    }

    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        if let Some(status) = &status {
            for line in &self.statuses {
                let _ = status.send(line.clone());
            }
        }
        self.dispatch(request).await
    }
}

/// A dispatch that always fails, for the unhappy path.
pub(super) struct BrokenHarness;

#[async_trait]
impl HarnessDispatch for BrokenHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(RunError::Worker("no harness installed".into()))
    }
}

/// A dispatch that fails with one chosen [`RunError`], for asserting that the
/// four kinds of failure stay distinguishable to the caller.
pub(super) struct FailingHarness(pub(super) RunError);

#[async_trait]
impl HarnessDispatch for FailingHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(self.0.clone())
    }
}

/// A store whose listing always fails.
///
/// Stands in for a workflows directory that has gone unreadable mid-session —
/// permissions changed, a mount dropped — which is the case where swallowing the
/// error used to make every existing workflow look newly created.
pub(super) struct UnreadableStore;

impl UnreadableStore {
    fn failure() -> WorkflowError {
        WorkflowError::Io {
            path: "workflows".into(),
            source: std::io::Error::other("unreadable"),
        }
    }
}

impl WorkflowStore for UnreadableStore {
    fn list(&self) -> Result<Vec<crate::workflows::WorkflowSummary>, WorkflowError> {
        Err(Self::failure())
    }
    fn get(&self, _id: &str) -> Result<Option<WorkflowRecord>, WorkflowError> {
        Err(Self::failure())
    }
    fn save(&self, _record: &WorkflowRecord) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn delete(&self, _id: &str) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn record_run(&self, _run: &crate::workflows::RunRecord) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn get_run(&self, _run_id: &str) -> Result<Option<crate::workflows::RunRecord>, WorkflowError> {
        Err(Self::failure())
    }
    fn list_runs(
        &self,
        _workflow_id: &str,
    ) -> Result<Vec<crate::workflows::RunRecord>, WorkflowError> {
        Err(Self::failure())
    }
    fn list_revisions(
        &self,
        _workflow_id: &str,
    ) -> Result<Vec<crate::workflows::WorkflowRevision>, WorkflowError> {
        Err(Self::failure())
    }
    fn revision(
        &self,
        _workflow_id: &str,
        _revision_id: &str,
    ) -> Result<Option<crate::workflows::WorkflowRevision>, WorkflowError> {
        Err(Self::failure())
    }
}

/// A store over a temporary directory, holding `sweep`.
pub(super) fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    store.save(&document("sweep")).expect("save");
    (root, store as Arc<dyn WorkflowStore>)
}

/// A session dispatching to `harness`.
pub(super) fn session(
    store: Arc<dyn WorkflowStore>,
    harness: Arc<dyn HarnessDispatch>,
) -> CopilotSession {
    CopilotSession {
        store,
        dispatch: harness,
        worker_address: "worker".into(),
        provider: None,
        model: None,
        conversation: "pane-1".into(),
        recap: None,
    }
}
