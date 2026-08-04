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
    let write_dir = catalog_identity(dirs);
    let scope = format!(
        "{:x}",
        Sha256::digest(write_dir.as_os_str().as_encoded_bytes())
    );
    state_root.join("definitions").join(scope)
}

/// Canonical identity of the catalog's write destination.
pub(crate) fn catalog_identity(dirs: &[PathBuf]) -> PathBuf {
    let raw = dirs.last().map_or_else(
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
    canonical_path_identity(&raw)
}

/// Resolve existing symlinks while retaining lexical semantics for missing parts.
fn canonical_path_identity(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if resolved.exists() {
                    resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
                }
                resolved.pop();
            }
            other => {
                resolved.push(other.as_os_str());
                if resolved.exists() {
                    resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
                }
            }
        }
    }
    resolved
}

#[cfg(test)]
#[path = "dirs_tests.rs"]
mod tests;
