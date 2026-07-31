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
/// directory without writing to the shared marker other processes read. It is
/// also how an install is returned to the pre-login home without signing in
/// (`MEDULLA_USER=local`), since nothing removes the marker: logout clears the
/// session and leaves the selection, so signing back in still finds the
/// account's own config and its backend.
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

/// The account each root has been pinned to for the life of this process.
///
/// The marker is shared mutable state: another process running `medulla login`
/// or `logout` rewrites it at any moment. Re-reading it on every call would let
/// that rewrite re-home a *running* process — the workflow store, the log
/// directory, and the identity would start resolving to account B while the core
/// this process booted, and the session it holds, stay account A's. A daemon
/// would then run B's workflows out of A's core.
///
/// So the first resolution for a root wins and is remembered. A process runs as
/// one account from start to finish; only this process changing the marker
/// itself (login, logout) moves it, and those update the pin deliberately.
///
/// Keyed by root rather than global because one process legitimately resolves
/// several: the test suite drives many scratch roots, and `MEDULLA_HOME`-pinned
/// tooling can address another install.
static PINNED: std::sync::Mutex<Option<HashMap<std::path::PathBuf, String>>> =
    std::sync::Mutex::new(None);

/// Run `f` against the pin table, recovering a poisoned lock.
///
/// A panic while the table is held says nothing about the table's contents — the
/// alternative is turning an unrelated panic in one thread into a permanently
/// unresolvable home in every other.
fn with_pins<T>(f: impl FnOnce(&mut HashMap<std::path::PathBuf, String>) -> T) -> T {
    let mut guard = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// The account id this process should scope to.
///
/// Precedence: `MEDULLA_USER` (env) → this process's pin for `root` → the marker
/// file under `root` → [`PRE_LOGIN_USER_ID`]. A blank, unreadable, unparseable,
/// or unsafe id reads as absent rather than failing: the fallback is a usable
/// signed-out home, and there is no caller in a position to handle an error
/// here.
///
/// The first call for a root fixes the answer for this process — see [`PINNED`].
/// `MEDULLA_USER` sits ahead of the pin because it is this process's own
/// environment, not shared state another process can change underneath it.
pub fn active_user_id(env: &HashMap<String, String>, root: &Path) -> String {
    if let Some(explicit) = env
        .get(MEDULLA_USER_ENV)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .and_then(sanitize)
    {
        return explicit;
    }
    with_pins(|pins| {
        if let Some(pinned) = pins.get(root) {
            return pinned.clone();
        }
        let resolved = read_active_user_id(root).unwrap_or_else(|| PRE_LOGIN_USER_ID.to_string());
        pins.insert(root.to_path_buf(), resolved.clone());
        resolved
    })
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
    // This process changed the marker, so its pin moves with it — that is the
    // difference between our own login and another process's, which must not
    // re-home us. `medulla login` resolves the home again straight after this to
    // boot the core against it, and would otherwise get the pre-login pin it
    // took while loading config.
    with_pins(|pins| pins.insert(root.to_path_buf(), user_id.clone()));
    tracing::debug!("[home] active user recorded as {user_id}");
    Ok(())
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
