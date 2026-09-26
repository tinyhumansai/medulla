//! Adopt the credential files written before the session store moved.
//!
//! Two stores predate [`super::session`]. A `credentials.json` under the Medulla
//! home (and an older one under the OS config directory) held a real JWT back
//! when Medulla owned its own credential file; it was retired when sessions
//! moved into the embedded OpenHuman core, and from then on `login` and `logout`
//! simply deleted it — a bearer token sitting in a file that no code path can
//! invalidate is strictly worse than one in use.
//!
//! Now that the store is Medulla's again, deleting is the wrong move: the token
//! in that file is the same app session the new store wants, so an install that
//! predates the core is signed in and should stay signed in. The sweep therefore
//! *adopts* — hand the JWT back to the caller, which verifies it through
//! [`super::session::store`] like any other, and remove the old file only once
//! the new one is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::Credentials;

/// The retired store under the Medulla home.
fn home_location(home: &Path) -> PathBuf {
    home.join("credentials.json")
}

/// A dedicated override for [`config_dir_location`] — deliberately not
/// `XDG_CONFIG_HOME`, which a real install can have set for reasons that have
/// nothing to do with Medulla (cross-platform dotfiles commonly export it even
/// on macOS and Windows, where the OS itself ignores it). Repurposing that
/// variable as the override would mean production adoption and sign-out only
/// ever look under it and never at `dirs::config_dir()`'s native location —
/// missing the real legacy file such an install actually has, silently, until
/// the variable is unset and the old bearer authenticates the operator again.
///
/// This key does not have that problem because nothing but a caller that
/// means to redirect this specific lookup would ever set it — in practice,
/// only a test, pointing the sweep at a scratch directory instead of the
/// developer's or CI runner's real one. `env` is the same injected map every
/// other resolution in this crate takes; the override is read from it, never
/// from the real process environment.
pub const LEGACY_CONFIG_DIR_OVERRIDE: &str = "MEDULLA_LEGACY_CONFIG_DIR";

/// The older retired store under the OS config directory.
///
/// [`dirs::config_dir`] on every platform, in production — untouched by this
/// PR's fix for the OS-config-directory sweep not being test-isolable, which
/// lives entirely in [`LEGACY_CONFIG_DIR_OVERRIDE`] instead: absent (every
/// real caller), this resolves exactly as it always did; present (a test),
/// it replaces `dirs::config_dir()` outright rather than adding to it, so a
/// test sees only the scratch location it set, never the real one too.
fn config_dir_location(env: &HashMap<String, String>) -> Option<PathBuf> {
    let base = match env
        .get(LEGACY_CONFIG_DIR_OVERRIDE)
        .filter(|s| !s.is_empty())
    {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()?,
    };
    Some(base.join("medulla").join("credentials.json"))
}

/// A legacy credential that is worth trying, with the file it came from.
#[derive(Debug, Clone)]
pub struct LegacyCredential {
    /// The file the credential was read from, so the caller can remove it once
    /// the token has been verified and re-stored.
    pub path: PathBuf,
    /// The backend origin recorded alongside the JWT.
    pub base_url: String,
    /// The bearer token itself.
    pub jwt: String,
}

/// Read any legacy credential file, newest location first.
///
/// Best-effort: an unreadable or malformed file is skipped rather than reported,
/// because the only recovery is to sign in again and failing the command that
/// the sweep rides along with would block exactly that. Returns every candidate
/// so a caller that finds the first token rejected can try the next.
///
/// `env` is consulted only for [`LEGACY_CONFIG_DIR_OVERRIDE`] — see
/// [`config_dir_location`] — every other input is `home`, already resolved by
/// the caller.
pub fn adopt_legacy_credentials(
    env: &HashMap<String, String>,
    home: &Path,
) -> Vec<LegacyCredential> {
    let mut found = Vec::new();
    for path in [Some(home_location(home)), config_dir_location(env)]
        .into_iter()
        .flatten()
    {
        let Ok(body) = std::fs::read(&path) else {
            continue;
        };
        let Ok(creds) = serde_json::from_slice::<Credentials>(&body) else {
            tracing::debug!("[auth] ignoring unreadable legacy credential at {path:?}");
            continue;
        };
        if creds.jwt.trim().is_empty() {
            continue;
        }
        found.push(LegacyCredential {
            path,
            base_url: creds.base_url,
            jwt: creds.jwt,
        });
    }
    found
}

/// Remove a legacy credential file once its token has been re-stored.
///
/// Idempotent and best-effort, for the same reason the read is: a file that
/// cannot be removed (permissions, a read-only mount) is reported by returning
/// `false` rather than by failing the caller's command.
pub fn discard_legacy_credential(path: &Path) -> bool {
    path.exists() && std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
