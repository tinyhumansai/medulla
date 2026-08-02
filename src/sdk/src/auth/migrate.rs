//! Remove the retired Medulla-owned credential files.
//!
//! Sessions live in the embedded OpenHuman core now. Installs that predate the
//! cutover still have a `credentials.json` holding a real JWT, and nothing reads
//! it any more — so nothing would ever delete it either. A bearer token sitting
//! in a file that no code path can invalidate is strictly worse than one in use,
//! so `login` and `logout` sweep it as they pass.

use std::path::{Path, PathBuf};

/// The retired store under the Medulla home.
fn home_location(home: &Path) -> PathBuf {
    home.join("credentials.json")
}

/// The older retired store under the OS config directory.
fn config_dir_location() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("medulla").join("credentials.json"))
}

/// Delete any retired credential file, returning the paths actually removed.
///
/// Best-effort and idempotent: a missing file is the normal case, and a file
/// that cannot be removed (permissions, a read-only mount) is reported by
/// omission rather than by failing the command the sweep is riding along with —
/// refusing to log out because a stale file could not be tidied would be the
/// wrong trade.
pub fn remove_retired_credentials(home: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for path in [Some(home_location(home)), config_dir_location()]
        .into_iter()
        .flatten()
    {
        if path.exists() && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}
