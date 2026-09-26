//! Resolving the Medulla root and the account-scoped home from an environment.
//!
//! Pure over an injected env map — apart from [`super::user`]'s marker read,
//! which is the one input that cannot come from the environment — so the whole
//! precedence chain is unit-testable without touching the real process
//! environment.

use std::collections::HashMap;
use std::path::PathBuf;

use super::user;

/// Whether an env value is truthy: `"1"` or `"true"` (case-insensitive, trimmed).
pub fn is_truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
}

/// Resolve the account-scoped Medulla home — `<root>/<user id>`.
///
/// This is what every caller that persists something wants. The account id
/// comes from [`user::active_user_id`]: the `MEDULLA_USER` override, else the
/// root's `active_user.toml` marker, else the pre-login id. A signed-out
/// install resolves to `<root>/local` rather than to nothing.
pub fn medulla_home(env: &HashMap<String, String>) -> PathBuf {
    let root = medulla_root(env);
    let user = user::active_user_id(env, &root);
    root.join(user)
}

/// Resolve the install-wide Medulla root (the directory holding every account).
///
/// Precedence:
/// 1. `MEDULLA_HOME` — an explicit path wins over everything.
/// 2. `MEDULLA_DEV` truthy — a local-dev root at `./.medulla` (relative to cwd).
/// 3. otherwise `<home>/.medulla`, where `<home>` comes from `HOME` /
///    `USERPROFILE` (or [`dirs::home_dir`] as a last resort).
///
/// Note that `MEDULLA_HOME` names the *root*, not one account's home: a scratch
/// run (`MEDULLA_HOME=$(mktemp -d)`) still gets its own account directory
/// underneath, and stays as isolated as it was before scoping existed.
pub fn medulla_root(env: &HashMap<String, String>) -> PathBuf {
    if let Some(explicit) = env
        .get("MEDULLA_HOME")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(explicit);
    }
    if env
        .get("MEDULLA_DEV")
        .map(|v| is_truthy(v))
        .unwrap_or(false)
    {
        return PathBuf::from(".medulla");
    }
    home_base(env).join(".medulla")
}

/// The user's OS home directory, from the injected env first (`HOME`, then
/// `USERPROFILE`), falling back to [`dirs::home_dir`] and finally `.`.
fn home_base(env: &HashMap<String, String>) -> PathBuf {
    if let Some(h) = env.get("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(h);
    }
    if let Some(h) = env.get("USERPROFILE").filter(|s| !s.is_empty()) {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}
