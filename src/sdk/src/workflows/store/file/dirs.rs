//! Which directories hold workflows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::home::medulla_home;

/// The workflow directories, lowest precedence first: project-local
/// `<cwd>/.medulla/workflows`, then user-global `<medulla home>/workflows`.
///
/// Project definitions remain readable as repository-provided defaults, while
/// authored and edited definitions are written to the final, user-global layer
/// beside the rest of Medulla's persistent data.
///
/// The two are always distinct directories. They used to be able to collapse
/// into one — under `MEDULLA_DEV=1` the home *was* `./.medulla`, and reading it
/// twice made every workflow shadow itself — but the home is now the account
/// directory one level inside the root (`./.medulla/<account>`), which no
/// project store can name.
pub fn workflow_dirs(env: &HashMap<String, String>, cwd: &Path) -> Vec<PathBuf> {
    vec![
        cwd.join(".medulla").join("workflows"),
        medulla_home(env).join("workflows"),
    ]
}

/// State shared by stores writing the same catalog, beneath the caller's root.
pub(crate) fn definition_state_dir(state_root: &Path, dirs: &[PathBuf]) -> PathBuf {
    let raw_write_dir = dirs.last().map_or_else(
        || PathBuf::from("."),
        |dir| {
            if dir.is_absolute() {
                dir.clone()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(dir)
            }
        },
    );
    let write_dir = canonical_path_identity(&raw_write_dir);
    let scope = format!(
        "{:x}",
        Sha256::digest(write_dir.as_os_str().as_encoded_bytes())
    );
    state_root.join("definitions").join(scope)
}

/// Resolve aliases in the deepest existing prefix, retaining a missing tail.
fn canonical_path_identity(path: &Path) -> PathBuf {
    let normalized = path
        .components()
        .fold(PathBuf::new(), |mut result, component| {
            match component {
                std::path::Component::ParentDir => {
                    result.pop();
                }
                std::path::Component::CurDir => {}
                other => result.push(other.as_os_str()),
            }
            result
        });
    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            break;
        };
        existing = parent;
    }
    let mut resolved = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    resolved
}

#[cfg(test)]
#[path = "dirs_tests.rs"]
mod tests;
