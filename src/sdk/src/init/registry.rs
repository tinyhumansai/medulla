//! Enrolling an initialised directory in the operator's workspace registry.
//!
//! Writing a `MEDULLA.md` describes a workspace; it does not make the
//! orchestrator aware of it. Two config lists do that, and `init` writes both:
//!
//! - `[workflow].workspaces` — the roots whose profiles ride every backend
//!   session mint (`workspaceProfiles`, see [`crate::init::collect_profile_inputs`]).
//!   Without an entry here the authored `MEDULLA.md` is never read at runtime.
//! - `[fleet].workspaces` — the declared `Host → Harness → Workspace` chain the
//!   core handshake carries and the Fleet page renders. Without an entry here
//!   the orchestrator has nowhere to *place* work it decides to route.
//!
//! Registration is an upsert keyed on the absolute path, so re-running `init`
//! on a directory refreshes it rather than duplicating it — and never clobbers
//! fields an operator hand-tuned (name, harness, templates, metadata).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::config::{persist_setting, persist_workflow_workspaces, TuiConfig};
use crate::runtime::WorkspaceDescriptor;

/// The harness a workspace is attached to when the operator has declared none.
///
/// A workspace must name an owning harness for the containment chain to be
/// well-formed. On a plain single-laptop install there is no `[fleet]` section
/// at all, so `init` attaches to this conventional local id rather than refusing
/// to register.
pub const DEFAULT_HARNESS_ID: &str = "local";

/// What [`register_workspace`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRegistration {
    /// The config file the registry entries were written to.
    pub config_path: PathBuf,
    /// The registry id of the workspace — stable across re-runs.
    pub id: String,
    /// Operator-facing name (the directory's own name).
    pub name: String,
    /// The absolute workspace path recorded in both lists.
    pub path: String,
    /// The harness the workspace was attached to.
    pub harness_id: String,
    /// True when this was a new entry, false when an existing one was refreshed.
    pub added: bool,
}

/// Resolve `dir` to the absolute path used as the registry key.
///
/// Canonicalised when the directory exists so that `.`, `..`, and symlinked
/// checkouts all resolve to one entry; a non-existent directory is an error,
/// because registering a path nothing can run in only produces a broken chain.
pub fn registry_path(dir: &Path) -> Result<String> {
    let resolved = dir
        .canonicalize()
        .map_err(|err| anyhow!("{}: {err}", dir.display()))?;
    Ok(resolved.display().to_string())
}

/// A stable, readable registry id for a workspace path.
///
/// The directory name carries the meaning (`ws-medulla-public`), and a short
/// hash of the absolute path disambiguates the several checkouts of the same
/// repository an operator is likely to have — worktrees especially, which share
/// a name by construction.
pub fn workspace_id(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug
    };
    format!("ws-{slug}-{}", short_hash(path))
}

/// A short, stable hex digest of `text` (FNV-1a, 64-bit, truncated).
///
/// Only used to disambiguate ids, so collision resistance matters far less than
/// being dependency-free and identical across runs and platforms.
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash >> 32) as u32)
}

/// The operator-facing name for a workspace path: the directory's own name.
fn workspace_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Which harness a newly registered workspace attaches to.
///
/// An explicit `--harness` wins. Otherwise the first harness the operator has
/// already declared is used — on a configured fleet that is the one that would
/// actually run the work — falling back to [`DEFAULT_HARNESS_ID`] when nothing
/// is declared at all.
fn resolve_harness_id(config: &TuiConfig, requested: Option<&str>) -> String {
    if let Some(id) = requested.map(str::trim).filter(|id| !id.is_empty()) {
        return id.to_string();
    }
    config
        .fleet
        .harnesses
        .first()
        .map(|h| h.id.clone())
        .unwrap_or_else(|| DEFAULT_HARNESS_ID.to_string())
}

/// Build the descriptor for a workspace that is not in the registry yet.
fn new_descriptor(path: &str, harness_id: &str) -> WorkspaceDescriptor {
    WorkspaceDescriptor {
        id: workspace_id(path),
        name: workspace_name(path),
        path: path.to_string(),
        harness_id: harness_id.to_string(),
        profile: None,
        project: detect_project(Path::new(path)),
        template_ids: Vec::new(),
        metadata: serde_json::Map::new(),
    }
}

/// The project identity for a directory: the name of the enclosing git
/// repository when there is one, else the directory's own name.
///
/// Detected by walking up for a `.git` entry (a file, for worktrees and
/// submodules; a directory otherwise) rather than shelling out to git, so this
/// stays cheap and works with no git binary installed.
fn detect_project(dir: &Path) -> Option<String> {
    let mut cursor = Some(dir);
    while let Some(current) = cursor {
        if current.join(".git").exists() {
            return current
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
        }
        cursor = current.parent();
    }
    None
}

/// Register `dir` in both workspace lists of the config file at `config_path`.
///
/// `config` is the *loaded* (layered) config, read to decide the harness and to
/// merge with what is already declared; `config_path` is the single file the
/// result is written to. When config is layered, a higher-precedence file can
/// still override what is written here — the caller should report which file it
/// targeted.
///
/// Idempotent: an existing entry for the same absolute path keeps its id and
/// every operator-tuned field, and only its path-derived fields are refreshed.
///
/// # Errors
///
/// Returns an error when `dir` does not exist or cannot be canonicalised, or
/// when the config file cannot be parsed or written.
pub fn register_workspace(
    config_path: &Path,
    config: &TuiConfig,
    dir: &Path,
    harness: Option<&str>,
) -> Result<WorkspaceRegistration> {
    let path = registry_path(dir)?;

    // --- [fleet].workspaces: the placement chain ---------------------------
    let mut workspaces = config.fleet.workspaces.clone();
    let existing = workspaces.iter().position(|w| w.path == path);
    let added = existing.is_none();
    let harness_id = match existing {
        // Re-running `init` must not silently re-home a workspace the operator
        // attached to a specific harness; only an explicit request moves it.
        Some(index) => harness
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| workspaces[index].harness_id.clone()),
        None => resolve_harness_id(config, harness),
    };

    match existing {
        Some(index) => {
            let entry = &mut workspaces[index];
            entry.harness_id = harness_id.clone();
            if entry.name.trim().is_empty() {
                entry.name = workspace_name(&path);
            }
            if entry.project.is_none() {
                entry.project = detect_project(Path::new(&path));
            }
        }
        None => workspaces.push(new_descriptor(&path, &harness_id)),
    }

    let id = workspaces
        .iter()
        .find(|w| w.path == path)
        .map(|w| w.id.clone())
        .unwrap_or_else(|| workspace_id(&path));
    let name = workspace_name(&path);

    persist_fleet_workspaces(config_path, &workspaces)?;

    // --- [workflow].workspaces: the profile-carrying roots -----------------
    let mut roots = config.workflow.workspaces.clone();
    if !roots.iter().any(|root| root == &path) {
        roots.push(path.clone());
    }
    persist_workflow_workspaces(config_path, &roots)?;

    Ok(WorkspaceRegistration {
        config_path: config_path.to_path_buf(),
        id,
        name,
        path,
        harness_id,
        added,
    })
}

/// Remove a workspace from both registry lists, by registry id or by path.
///
/// `selector` is matched against the entry's id, its recorded path, and — when
/// the selector names a directory that exists — its canonicalised path, so
/// `medulla workspace remove .` removes the entry `medulla init` made from the
/// same directory.
///
/// Removing only unregisters: the directory and its `MEDULLA.md` are left alone,
/// because the operator asked the orchestrator to stop placing work there, not
/// to lose the profile they authored.
///
/// # Errors
///
/// Returns an error when nothing matches `selector`, or when the config file
/// cannot be parsed or written.
pub fn deregister_workspace(
    config_path: &Path,
    config: &TuiConfig,
    selector: &str,
) -> Result<WorkspaceRegistration> {
    // A selector that names a real directory is compared canonically; one that
    // does not is still a valid id or a stale path, so a failure here is not an
    // error on its own.
    let canonical = registry_path(Path::new(selector)).ok();
    let matches = |candidate: &str| {
        candidate == selector || canonical.as_deref().is_some_and(|path| candidate == path)
    };

    let mut workspaces = config.fleet.workspaces.clone();
    let index = workspaces
        .iter()
        .position(|w| w.id == selector || matches(&w.path));

    let removed = match index {
        Some(index) => workspaces.remove(index),
        None => {
            // Not in the fleet chain, but possibly still a profile-carrying root
            // — a half-registered workspace must still be removable.
            let root = config
                .workflow
                .workspaces
                .iter()
                .find(|root| matches(root))
                .ok_or_else(|| anyhow!("no registered workspace matches '{selector}'"))?;
            new_descriptor(root, DEFAULT_HARNESS_ID)
        }
    };

    persist_fleet_workspaces(config_path, &workspaces)?;

    let roots: Vec<String> = config
        .workflow
        .workspaces
        .iter()
        .filter(|root| !matches(root) && *root != &removed.path)
        .cloned()
        .collect();
    persist_workflow_workspaces(config_path, &roots)?;

    Ok(WorkspaceRegistration {
        config_path: config_path.to_path_buf(),
        id: removed.id,
        name: removed.name,
        path: removed.path,
        harness_id: removed.harness_id,
        added: false,
    })
}

/// Replace the `[fleet].workspaces` array, preserving every other key.
///
/// Serialised through the descriptor's own serde representation so the file
/// shape and the wire shape cannot drift apart.
fn persist_fleet_workspaces(path: &Path, workspaces: &[WorkspaceDescriptor]) -> Result<()> {
    let value = toml::Value::try_from(workspaces)
        .map_err(|err| anyhow!("Cannot serialize workspaces: {err}"))?;
    persist_setting(path, "fleet", "workspaces", value)
}
