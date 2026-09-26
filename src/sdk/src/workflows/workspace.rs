//! Where a run executes: resolving the caller-supplied workspace.
//!
//! A run's workspace is the directory every `medulla:shell` step runs in, the
//! root its `args.cwd` and `args.script_path` are resolved inside (see
//! [`crate::flow_engine::caps::script_policy`]), and the checkout every `agent`
//! node's harness session is opened on. Until this module existed it was always
//! the directory the caller happened to be sitting in, so a workflow aimed at
//! another repository had to smuggle the path in as a declared input — and then
//! hit the script policy, which refuses anything outside the workspace.
//!
//! Making it a run parameter instead moves the decision to where it belongs:
//! *which* checkout is a property of this run, not of the workflow document,
//! and not of the shell the daemon was started from.
//!
//! Nothing here is a sandbox. The workspace bounds a step's paths *relative to
//! a root the caller named*; naming a different root is exactly what this
//! parameter is for, and a caller who can start a run can already run code with
//! the daemon's privileges (`workflows.allowCode`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::workflows::WorkflowError;

/// Resolve the workspace a run should execute in.
///
/// `requested` is the caller's optional `workspace` parameter: absolute, or
/// relative to `session_cwd`, with a leading `~` expanded from `HOME` because a
/// value arriving through JSON never passed a shell that would have done it.
/// `None`, or a blank string, keeps the historical behaviour — the run executes
/// in `session_cwd`, unchecked, which is the directory the caller is already in.
///
/// The answer is canonicalized, so a run record naming its workspace names a
/// real path rather than whatever mixture of symlinks and `..` the caller typed.
///
/// # Errors
///
/// Returns [`WorkflowError::Engine`] when a named workspace does not exist, is
/// not a directory, or cannot be canonicalized. Refusing here is deliberate: a
/// mistyped path that silently fell back to the session's own directory would
/// run the workflow against the wrong repository, which is the one outcome
/// worse than not running it at all.
pub fn resolve(
    requested: Option<&str>,
    session_cwd: &Path,
    env: &HashMap<String, String>,
) -> Result<PathBuf, WorkflowError> {
    let Some(raw) = requested.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(session_cwd.to_path_buf());
    };

    let expanded = expand_home(raw, env);
    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        session_cwd.join(expanded)
    };

    if !candidate.exists() {
        return Err(refused(format!(
            "workspace '{raw}' does not exist (resolved to {}); pass a directory on this host, \
             absolute or relative to where the run was started",
            candidate.display()
        )));
    }
    if !candidate.is_dir() {
        return Err(refused(format!(
            "workspace '{raw}' is not a directory (resolved to {})",
            candidate.display()
        )));
    }
    candidate.canonicalize().map_err(|err| {
        refused(format!(
            "workspace '{raw}' ({}) is unreadable: {err}",
            candidate.display()
        ))
    })
}

/// Expand a leading `~` against `HOME`, leaving every other path untouched.
///
/// `~user` is deliberately *not* expanded: resolving another account's home
/// needs a passwd lookup, and a path that looks expanded but is not would be
/// worse than one that plainly fails to exist.
fn expand_home(raw: &str, env: &HashMap<String, String>) -> PathBuf {
    let home = || {
        env.get("HOME")
            .map(String::as_str)
            .filter(|home| !home.trim().is_empty())
            .map(PathBuf::from)
    };
    match raw {
        "~" => home().unwrap_or_else(|| PathBuf::from(raw)),
        _ => match raw.strip_prefix("~/") {
            Some(rest) => match home() {
                Some(home) => home.join(rest),
                None => PathBuf::from(raw),
            },
            None => PathBuf::from(raw),
        },
    }
}

/// Every refusal here is the caller's own argument, so it reports as an engine
/// error carrying a sentence the caller can act on.
fn refused(message: String) -> WorkflowError {
    WorkflowError::Engine(message)
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
