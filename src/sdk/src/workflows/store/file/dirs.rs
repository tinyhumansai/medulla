//! Which directories hold workflows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
