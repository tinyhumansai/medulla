//! Which directories hold workflows, and when two of them are really one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::home::medulla_home;

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
