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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tinyflows::model::WorkflowGraph;

use crate::home::medulla_home;
use crate::workflows::types::{
    RunRecord, RunStatus, WorkflowError, WorkflowRecord, WorkflowSummary,
};

use super::WorkflowStore;

/// Suffix appended while writing, then renamed over the target. Matches the
/// idiom already used for trust state, so a half-written file is never
/// observable — and never mistaken for a definition, since the resulting
/// extension is not `json`.
const TMP_SUFFIX: &str = ".medulla-tmp";

/// The workflow directories, lowest precedence first: the user-global
/// `<medulla home>/workflows`, then the project-local `<cwd>/.medulla/workflows`.
///
/// The two collapse to one entry when they resolve to the same directory, the
/// normal case under `MEDULLA_DEV=1` (whose home *is* `./.medulla`) — reading it
/// twice would make every workflow shadow itself.
pub fn workflow_dirs(env: &HashMap<String, String>, cwd: &Path) -> Vec<PathBuf> {
    let home = medulla_home(env).join("workflows");
    let project = cwd.join(".medulla").join("workflows");
    let mut dirs = vec![home.clone()];
    if !same_dir(&home, &project) {
        dirs.push(project);
    }
    dirs
}

/// Whether two paths name the same directory, comparing canonical forms when
/// both exist and `.`-insensitive components otherwise.
///
/// The textual comparison matters precisely where canonicalization cannot help:
/// under `MEDULLA_DEV=1` the home is the relative `./.medulla` and the directory
/// may not exist yet, so `.medulla/workflows` and `./.medulla/workflows` are one
/// directory that no filesystem call will confirm.
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b || normalized(a) == normalized(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A path's components with no-op `.` segments dropped.
fn normalized(path: &Path) -> Vec<std::path::Component<'_>> {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

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
        let path = match existing.source_path {
            Some(path) => path,
            None => self.definition_path(id)?,
        };
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
}

/// An identifier's use as a single filename component, or an error.
///
/// Workflow ids and run ids both become filenames, and both are attacker-shaped
/// input: a workflow document's `id` overrides whatever the caller asked for, a
/// document may be written by an agent, and a run id can arrive on a task frame
/// from a peer. Without this, an id of `../../authorized_keys` would let a save
/// write outside the workflow directory with the daemon's privileges.
///
/// The rule is deliberately strict rather than sanitizing: an id that is not
/// already a safe component is rejected, not silently rewritten into a
/// different one. Rewriting would let two distinct ids collapse onto one file.
pub fn safe_component(id: &str) -> Result<&str, WorkflowError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(WorkflowError::Malformed(
            "identifier must not be empty".to_string(),
        ));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(WorkflowError::Malformed(format!(
            "identifier '{trimmed}' is not a usable filename"
        )));
    }
    // Both separators, on every platform: a document written on one machine is
    // read on another, and `a\..\b` must not become traversal on Windows just
    // because it was authored on unix.
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
        return Err(WorkflowError::Malformed(format!(
            "identifier '{trimmed}' must not contain a path separator"
        )));
    }
    // Catches drive-relative and other platform spellings the checks above miss
    // by asking the platform itself whether this is one plain component.
    let path = Path::new(trimmed);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(trimmed),
        _ => Err(WorkflowError::Malformed(format!(
            "identifier '{trimmed}' must be a single path component"
        ))),
    }
}

/// Whether a path is a file this store reads.
fn is_json(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("json"))
            .unwrap_or(false)
}

/// Read and parse one workflow document, naming errors by path.
fn read_workflow(path: &Path) -> Result<WorkflowRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut record =
        parse_workflow(&text, stem).map_err(|err| format!("{}: {err}", path.display()))?;
    // A document can deserialize cleanly and still be a graph the engine will
    // not compile — no trigger, an edge to a node that is not there. Catching
    // it here means a listing only ever shows workflows that would actually
    // run, and the operator hears about the broken file by name.
    validate_graph(&record.id, &record.graph)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    record.source_path = Some(path.to_path_buf());
    Ok(record)
}

/// Parse one workflow document.
///
/// The document is the engine's `WorkflowGraph` JSON with optional host fields
/// (`description`, `enabled`) alongside it. `id` defaults to `id_fallback` — the
/// filename, for a file — and `name` to the id, so the smallest useful document
/// is a set of nodes and edges.
///
/// The pipeline is the engine's documented one: migrate the persisted JSON to
/// the current schema *before* deserializing, so a definition saved by an older
/// build keeps loading.
pub fn parse_workflow(text: &str, id_fallback: &str) -> Result<WorkflowRecord, String> {
    let raw: Value = serde_json::from_str(text).map_err(|err| format!("invalid JSON: {err}"))?;
    let migrated = tinyflows::migrate::migrate(raw).map_err(|err| err.to_string())?;

    let object = migrated
        .as_object()
        .ok_or_else(|| "workflow document must be a JSON object".to_string())?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(id_fallback)
        .to_string();
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let graph: WorkflowGraph =
        serde_json::from_value(migrated).map_err(|err| format!("invalid workflow: {err}"))?;
    let name = if graph.name.is_empty() {
        id.clone()
    } else {
        graph.name.clone()
    };

    Ok(WorkflowRecord {
        id,
        name,
        description,
        enabled,
        graph,
        source_path: None,
    })
}

/// Run the engine's validation, collecting every failure rather than the first.
///
/// One round-trip then tells an author everything wrong with their graph, which
/// matters most when the author is an agent editing over a tool call.
pub fn validate_graph(id: &str, graph: &WorkflowGraph) -> Result<(), WorkflowError> {
    let errors = tinyflows::validate::validate_all(graph);
    if errors.is_empty() {
        return Ok(());
    }
    Err(WorkflowError::Invalid {
        id: id.to_string(),
        messages: errors
            .iter()
            .map(|err| match err.node_id() {
                Some(node) => format!("[{}] {node}: {err}", err.code()),
                None => format!("[{}] {err}", err.code()),
            })
            .collect(),
    })
}

/// Serialize a record into the on-disk document shape: the graph, with the host
/// fields merged in beside it.
fn to_document(record: &WorkflowRecord) -> Result<Vec<u8>, WorkflowError> {
    let mut value = serde_json::to_value(&record.graph)
        .map_err(|err| WorkflowError::Malformed(err.to_string()))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), Value::String(record.id.clone()));
        object.insert("name".into(), Value::String(record.name.clone()));
        object.insert(
            "description".into(),
            Value::String(record.description.clone()),
        );
        object.insert("enabled".into(), Value::Bool(record.enabled));
    }
    serde_json::to_vec_pretty(&value).map_err(|err| WorkflowError::Malformed(err.to_string()))
}

/// Write `body` to `path` through a temporary file in the same directory, so a
/// reader never observes a half-written document.
fn write_atomic(path: &Path, body: &[u8]) -> Result<(), WorkflowError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| WorkflowError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // Appended rather than substituted for the extension, so an id containing a
    // dot cannot collide with a different workflow's temporary file — and
    // carrying a unique token, so two writers racing on the *same* id cannot
    // scribble over each other's scratch file before either rename lands.
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!("{TMP_SUFFIX}.{}", uuid::Uuid::new_v4()));
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, body).map_err(|source| WorkflowError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| WorkflowError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Add `record` to `workflows`, replacing any entry with the same id in place so
/// a project-local override keeps the position of what it overrides.
fn upsert(workflows: &mut Vec<WorkflowRecord>, record: WorkflowRecord) {
    match workflows.iter_mut().find(|w| w.id == record.id) {
        Some(existing) => *existing = record,
        None => workflows.push(record),
    }
}

/// A run record for a run that has just started.
pub fn new_run_record(id: &str, workflow_id: &str, started_at: u64) -> RunRecord {
    RunRecord {
        id: id.to_string(),
        workflow_id: workflow_id.to_string(),
        status: RunStatus::Running,
        started_at,
        finished_at: None,
        steps: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
    }
}
