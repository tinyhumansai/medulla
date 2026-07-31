//! The active-user marker: which account's directory this process reads.
//!
//! Everything Medulla persists is scoped to one account, under
//! `<root>/<user id>` (see [`super::medulla_home`]). The id itself cannot come
//! from the account store — that store lives *inside* the scoped directory, so
//! reading it would require knowing the answer first. It comes from a marker
//! file at the root instead, written by the login flow and cleared by logout.
//!
//! This mirrors how the embedded OpenHuman core scopes its own state
//! (`~/.openhuman/active_user.toml` → `users/<id>/`), for the same reason: one
//! small file at a fixed path is the only thing a process can read before it
//! knows who it is.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// File name of the root-level marker naming the active account.
pub const ACTIVE_USER_FILE: &str = "active_user.toml";

/// Environment override for the active account id, ahead of the marker file.
///
/// A test harness or an operator juggling two accounts sets this to pick a
/// directory without writing to the shared marker other processes read.
pub const MEDULLA_USER_ENV: &str = "MEDULLA_USER";

/// The id used before anyone has signed in.
///
/// Pre-login state is real state — a config file, logs, a workflow store — and
/// it has to live somewhere. It lives under its own id rather than at the root
/// so that the root holds nothing but per-account directories and the marker
/// that selects between them.
pub const PRE_LOGIN_USER_ID: &str = "local";

/// Serde shape of the marker file.
#[derive(serde::Serialize, serde::Deserialize)]
struct ActiveUser {
    user_id: String,
}

/// Path of the marker for a given root.
pub fn active_user_path(root: &Path) -> PathBuf {
    root.join(ACTIVE_USER_FILE)
}

/// The account id this process should scope to.
///
/// Precedence: `MEDULLA_USER` (env) → the marker file under `root` →
/// [`PRE_LOGIN_USER_ID`]. A blank, unreadable, unparseable, or unsafe id reads
/// as absent rather than failing: the fallback is a usable signed-out home, and
/// there is no caller in a position to handle an error here.
pub fn active_user_id(env: &HashMap<String, String>, root: &Path) -> String {
    if let Some(explicit) = env
        .get(MEDULLA_USER_ENV)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .and_then(sanitize)
    {
        return explicit;
    }
    read_active_user_id(root).unwrap_or_else(|| PRE_LOGIN_USER_ID.to_string())
}

/// The account id recorded in `root`'s marker, if there is a usable one.
pub fn read_active_user_id(root: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(active_user_path(root)).ok()?;
    let parsed: ActiveUser = toml::from_str(&contents).ok()?;
    sanitize(parsed.user_id.trim())
}

/// Record `user_id` as the active account for `root`.
///
/// Written atomically (temp file + rename) because every later launch resolves
/// its entire home from this one value: a half-written marker would send the
/// next run to a directory that is not any account's.
///
/// # Errors
///
/// The id is not a usable directory name, the root cannot be created, or the
/// write/rename fails.
pub fn write_active_user_id(root: &Path, user_id: &str) -> std::io::Result<()> {
    let user_id = sanitize(user_id.trim()).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to record {user_id:?} as the active user id"),
        )
    })?;
    std::fs::create_dir_all(root)?;

    let body = toml::to_string_pretty(&ActiveUser {
        user_id: user_id.clone(),
    })
    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

    let temp = root.join(format!(".{ACTIVE_USER_FILE}.tmp-{}", std::process::id()));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    drop(file);

    if let Err(err) = std::fs::rename(&temp, active_user_path(root)) {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    tracing::debug!("[home] active user recorded as {user_id}");
    Ok(())
}

/// Forget the active account, sending the next launch back to the pre-login
/// home. Idempotent: clearing when nothing is recorded succeeds.
///
/// # Errors
///
/// The marker exists but cannot be removed.
pub fn clear_active_user(root: &Path) -> std::io::Result<()> {
    let path = active_user_path(root);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::debug!("[home] active user cleared");
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Accept an id only if it is safe to use as a single directory name.
///
/// The id arrives from a backend response and is turned straight into a path,
/// so anything that could escape the root — a separator, a `..`, a leading dot
/// (which would collide with the marker's own dotfiles) — is rejected rather
/// than sanitized into something that silently addresses a different account.
fn sanitize(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.starts_with('.') {
        return None;
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    ok.then(|| id.to_string())
}
