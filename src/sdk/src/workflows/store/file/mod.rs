//! JSON workflow documents under `.medulla/workflows`, one graph per file.
//!
//! The directory layering, the forgiving read, and the atomic write all match
//! how agent templates are already kept ([`crate::agents`]) — an operator who
//! has learned one has learned the other. The format is JSON rather than the
//! TOML used for templates because a node's `config` is free-form JSON that the
//! engine hands to jq expressions; round-tripping it through TOML would change
//! what the author wrote.
//!
//! Reading never fails as a whole. A missing directory is the normal state, and
//! a malformed document costs only itself — an operator hand-editing a catalog
//! should lose the file they broke, not the nine that are fine. What went wrong
//! travels back in [`LoadReport::errors`].
//!
//! The work is split by responsibility: [`dirs`] decides where to look,
//! [`document`] turns bytes into a record and back, [`paths`] guards the
//! identifier-to-filename boundary, and [`revisions`] keeps the superseded
//! copies that make an edit undoable. This module is the store itself.

mod dirs;
mod document;
mod paths;
mod revisions;

pub use dirs::workflow_dirs;
pub use document::{new_run_record, parse_workflow, validate_graph};
pub use revisions::MAX_REVISIONS;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::home::medulla_home;
use crate::workflows::types::{
    RunRecord, WorkflowError, WorkflowRecord, WorkflowRevision, WorkflowSummary,
};

use document::{read_workflow, to_document};
use paths::{is_json, write_atomic};
// Re-exported within the crate rather than merely imported: the identifier
// guard is the one piece of this module worth asserting on from outside it.
pub(crate) use paths::safe_component;

use super::WorkflowStore;

/// What one read of the workflow directories found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    /// Workflows in load order, later directories having replaced earlier ones
    /// of the same id.
    pub workflows: Vec<WorkflowRecord>,
    /// Directories that existed and were read, in precedence order.
    pub dirs: Vec<PathBuf>,
    /// One message per document that could not be read, parsed, or validated.
    pub errors: Vec<String>,
}

/// A workflow store backed by JSON files in the layered workflow directories.
#[derive(Debug, Clone)]
pub struct FileWorkflowStore {
    /// Definition directories, lowest precedence first.
    dirs: Vec<PathBuf>,
    /// Where run records are written. Runs are host state, not an authored
    /// artifact, so they live under the state directory rather than beside the
    /// definitions an operator edits.
    runs_dir: PathBuf,
}

impl FileWorkflowStore {
    /// A store over explicit directories. Mostly for tests; production callers
    /// want [`FileWorkflowStore::discover`].
    pub fn new(dirs: Vec<PathBuf>, runs_dir: PathBuf) -> Self {
        Self { dirs, runs_dir }
    }

    /// A store over the conventional locations for this environment and working
    /// directory.
    pub fn discover(env: &HashMap<String, String>, cwd: &Path) -> Self {
        let runs_dir = medulla_home(env)
            .join("state")
            .join("workflows")
            .join("runs");
        Self::new(workflow_dirs(env, cwd), runs_dir)
    }

    /// The definition directories, lowest precedence first.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The directory new definitions are written to: the highest-precedence one,
    /// which is the project-local directory when there is one.
    ///
    /// Writing to the most specific directory is what makes a saved workflow
    /// belong to the repository an agent is working in rather than leaking into
    /// every other checkout on the machine.
    pub fn write_dir(&self) -> &Path {
        self.dirs
            .last()
            .map(PathBuf::as_path)
            .unwrap_or_else(|| Path::new("."))
    }

    /// Read every `*.json` in every directory, later directories overriding
    /// earlier ones by workflow id.
    ///
    /// Files within one directory are read in sorted order so the catalog is
    /// stable across platforms. Never fails: a missing directory yields nothing
    /// and a bad document yields an entry in [`LoadReport::errors`].
    pub fn load(&self) -> LoadReport {
        let mut report = LoadReport::default();
        for dir in &self.dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                // Not existing is the normal state, not a failure worth reporting.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    report.errors.push(format!("{}: {err}", dir.display()));
                    continue;
                }
            };
            report.dirs.push(dir.clone());

            let mut paths: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|path| is_json(path))
                .collect();
            paths.sort();

            for path in paths {
                match read_workflow(&path) {
                    Ok(record) => upsert(&mut report.workflows, record),
                    Err(err) => report.errors.push(err),
                }
            }
        }
        report
    }

    /// The path a workflow with `id` is written to.
    fn definition_path(&self, id: &str) -> Result<PathBuf, WorkflowError> {
        Ok(self
            .write_dir()
            .join(format!("{}.json", safe_component(id)?)))
    }

    /// The version a save to `path` is about to supersede, if there is one.
    ///
    /// Two cases, and the cheap one is the common one. When the write directory
    /// already holds this workflow, that file *is* what a reader resolves to —
    /// the write directory is the highest-precedence one — so parsing it alone
    /// is exactly right and costs one read.
    ///
    /// Only when it does not is a full load needed: the workflow is coming from
    /// a lower-precedence directory and this save will shadow it. Snapshotting
    /// the shadowed version is what lets an operator undo a project-local edit
    /// back to what their home directory had.
    fn superseded_by(
        &self,
        path: &Path,
        id: &str,
    ) -> Result<Option<WorkflowRecord>, WorkflowError> {
        if path.exists() {
            // A file that no longer parses is not a version worth keeping, and
            // refusing the save over it would strand the operator with a broken
            // definition they cannot overwrite.
            return Ok(read_workflow(path).ok());
        }
        self.get(id)
    }

    /// The path a run record is written to.
    fn run_path(&self, run_id: &str) -> Result<PathBuf, WorkflowError> {
        Ok(self
            .runs_dir
            .join(format!("{}.json", safe_component(run_id)?)))
    }
}

impl WorkflowStore for FileWorkflowStore {
    fn list(&self) -> Result<Vec<WorkflowSummary>, WorkflowError> {
        Ok(self
            .load()
            .workflows
            .iter()
            .map(WorkflowRecord::summary)
            .collect())
    }

    fn get(&self, id: &str) -> Result<Option<WorkflowRecord>, WorkflowError> {
        Ok(self.load().workflows.into_iter().find(|w| w.id == id))
    }

    fn save(&self, record: &WorkflowRecord) -> Result<(), WorkflowError> {
        // The id decides a filename, so it is checked before anything else:
        // a document's own `id` overrides what the caller asked for, and a
        // document may have been written by an agent.
        let path = self.definition_path(&record.id)?;
        // Validate before writing so a listing can be trusted to be runnable.
        validate_graph(&record.id, &record.graph)?;
        let document = to_document(record)?;
        // Snapshot what is about to be replaced, before replacing it. Doing it
        // here rather than at each call site is what makes every authoring
        // surface undoable without any of them having to opt in.
        if let Some(superseded) = self.superseded_by(&path, &record.id)? {
            revisions::capture(self.write_dir(), &superseded)?;
        }
        write_atomic(&path, &document)
    }

    fn delete(&self, id: &str) -> Result<(), WorkflowError> {
        // Delete wherever it was found, not only in the write directory: an
        // operator asking to remove a workflow they can see means the one they
        // can see.
        let existing = self
            .load()
            .workflows
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| WorkflowError::NotFound(id.to_string()))?;
        let path = match existing.source_path.clone() {
            Some(path) => path,
            None => self.definition_path(id)?,
        };
        // Snapshot before removing. A delete is the one edit that leaves
        // nothing to diff against afterwards, so without this it is the one
        // edit that cannot be undone.
        revisions::capture(self.write_dir(), &existing)?;
        std::fs::remove_file(&path).map_err(|source| WorkflowError::Io { path, source })
    }

    fn record_run(&self, run: &RunRecord) -> Result<(), WorkflowError> {
        let path = self.run_path(&run.id)?;
        let body = serde_json::to_vec_pretty(run)
            .map_err(|err| WorkflowError::Malformed(err.to_string()))?;
        write_atomic(&path, &body)
    }

    fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, WorkflowError> {
        let path = self.run_path(run_id)?;
        let body = match std::fs::read(&path) {
            Ok(body) => body,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(WorkflowError::Io { path, source }),
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|err| WorkflowError::Malformed(format!("{}: {err}", path.display())))
    }

    fn list_runs(&self, workflow_id: &str) -> Result<Vec<RunRecord>, WorkflowError> {
        let entries = match std::fs::read_dir(&self.runs_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(WorkflowError::Io {
                    path: self.runs_dir.clone(),
                    source,
                })
            }
        };

        let mut runs: Vec<RunRecord> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| is_json(path))
            // A run record this host cannot parse is skipped rather than
            // failing the listing: history is diagnostic, and one corrupt file
            // should not hide the rest of it.
            .filter_map(|path| std::fs::read(&path).ok())
            .filter_map(|body| serde_json::from_slice::<RunRecord>(&body).ok())
            .filter(|run| run.workflow_id == workflow_id)
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        Ok(runs)
    }

    fn list_revisions(&self, workflow_id: &str) -> Result<Vec<WorkflowRevision>, WorkflowError> {
        revisions::list(self.write_dir(), workflow_id)
    }

    fn revision(
        &self,
        workflow_id: &str,
        revision_id: &str,
    ) -> Result<Option<WorkflowRevision>, WorkflowError> {
        revisions::read(self.write_dir(), workflow_id, revision_id)
    }
}

/// Add `record` to `workflows`, replacing any entry with the same id in place so
/// a project-local override keeps the position of what it overrides.
fn upsert(workflows: &mut Vec<WorkflowRecord>, record: WorkflowRecord) {
    match workflows.iter_mut().find(|w| w.id == record.id) {
        Some(existing) => *existing = record,
        None => workflows.push(record),
    }
}
