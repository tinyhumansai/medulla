//! The Medulla-owned app session store: where a verified JWT lives between runs.
//!
//! # Why this is here again
//!
//! Sessions used to live in the embedded OpenHuman core, reached through
//! `core_host::auth` over four `openhuman.auth_*` RPCs. That was the right
//! answer while the core *was* the runtime: two stores meant two answers to "am
//! I signed in?", and `medulla login` could succeed while the core — whose
//! session actually drove the runtime — stayed signed out.
//!
//! With the core gone there is no second store to disagree with, so the reason
//! for deferring outward is gone with it. This module is the single place a
//! session lives, which is the same invariant, now satisfied by owning the store
//! rather than by delegating it.
//!
//! # The store is validated on write, not on read
//!
//! [`store`] calls `GET /auth/me` with the candidate token and refuses to
//! persist one the backend rejects — the same contract
//! `openhuman.auth_store_session` had, and the reason a bad paste fails at
//! `medulla login` instead of on every later call. Reads are then cheap and
//! offline: a token that has since expired surfaces as a 401 from the call that
//! used it, which is where the caller can act on it.
//!
//! # Where the file goes
//!
//! `<root>/<account id>/session.json`, the account-scoped home from
//! [`crate::home::medulla_home`]. [`store`] resolves the account id from the
//! `/auth/me` response it just verified and publishes it as the active account
//! before writing, so the file lands under the account it belongs to rather than
//! under whichever one happened to be active. On unix the file is created 0600.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::LoginError;

/// The session file's name inside an account home.
const SESSION_FILE: &str = "session.json";

/// Who this install is signed in as.
///
/// Field-for-field the projection `core_host::auth::AuthState` presented, so the
/// TUI's account surface is unchanged by the move.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthState {
    /// Whether a usable session is stored.
    pub is_authenticated: bool,
    /// The signed-in account's id, when one is known.
    pub user_id: Option<String>,
}

/// The persisted session record.
///
/// `base_url` is recorded for diagnostics — which deployment issued this token —
/// and deliberately **not** matched against the configured backend on read. That
/// comparison used to exist and rejected perfectly good tokens whenever the two
/// spellings drifted (a trailing slash was enough), which is a worse failure
/// than the mismatch it guarded against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    /// The verified bearer token.
    pub token: String,
    /// The account id from the `/auth/me` response that verified the token.
    #[serde(default)]
    pub user_id: Option<String>,
    /// The backend origin that issued the token.
    #[serde(default)]
    pub base_url: String,
}

/// The session file for an account home.
fn location(home: &Path) -> PathBuf {
    home.join(SESSION_FILE)
}

/// Verify `jwt` against the backend and persist it as this install's session.
///
/// The token is checked with `GET /auth/me` before anything is written, so a
/// token for a different deployment — or an expired one — fails here rather than
/// being stored and failing on every later call. The account id in that response
/// becomes the active account, so the session file lands in that account's home.
///
/// # Errors
///
/// [`LoginError::Backend`] when `/auth/me` rejects the token or cannot be
/// reached, and [`LoginError::Io`] when the account home or the file itself
/// cannot be written.
pub async fn store(
    env: &HashMap<String, String>,
    base_url: &str,
    jwt: &str,
) -> Result<AuthState, LoginError> {
    let me = crate::client::MedullaClient::new(base_url, jwt)
        .me()
        .await
        .map_err(|e| {
            LoginError::Backend(format!("session validation failed (GET /auth/me): {e}"))
        })?;

    // An id-less response cannot be stored safely. Every guard below is keyed on
    // knowing whose token this is: the `MEDULLA_USER` conflict check, the
    // account switch, and the choice of home. With `None` all three are skipped
    // and the bearer lands in whichever account happens to be selected — which
    // `adopt_legacy` reaches directly, so a token from the *global* retired
    // credential file could be persisted under an unrelated account.
    //
    // Refusing costs a deployment that genuinely returns no id the ability to
    // log in, which is the right trade: a session nobody can attribute is one
    // this host cannot place, and placing it anyway is how two accounts end up
    // sharing one bearer.
    let user_id = crate::auth::user_id_from_me(&me).ok_or_else(|| {
        LoginError::Backend(
            "the backend accepted the token but named no account for it, so there is no \
             home to store it under"
                .to_string(),
        )
    })?;

    // Sanitized here, before anything is resolved or written, because the id is
    // about to become a directory name. An id like `../other` is non-empty, so
    // it passes the check above and then fails only at the marker write — after
    // the session has already been written into whichever account was selected,
    // replacing a valid one. `adopt_legacy` calls `store` directly, so that is
    // reachable without a login.
    //
    // A previous version of this deferred the sanitize and called the fallback
    // unreachable "because the conflict check already sanitized it". That check
    // runs only when `MEDULLA_USER` is set, so with no override nothing had.
    let account = crate::home::user::sanitize_account_id(&user_id).ok_or_else(|| {
        LoginError::Backend(format!(
            "the backend named account {user_id:?}, which cannot be used as a directory \
             name, so there is no home to store the session under"
        ))
    })?;
    let root = crate::home::medulla_root(env);

    // `MEDULLA_USER` suppresses the account switch entirely. That variable is an explicit
    // operator override of which account this process runs as, and
    // `active_user_id` already gives it precedence over the marker — so writing
    // the marker here would not change where *this* process writes, but it would
    // silently re-home every later launch that does not export the variable.
    // An override is a statement about one process, not a change of default.
    let pinned = env
        .get("MEDULLA_USER")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty());

    // A pinned account and a token belonging to a different one is a conflict,
    // not a precedence question. Suppressing the marker write is right — the
    // override is a statement about this process — but the home below still
    // resolves to the *pinned* account, so continuing would write the other
    // account's bearer into it. Every later run scoped to the pinned account
    // would then authenticate as somebody else, with nothing on screen saying
    // so. Refuse instead, naming both sides.
    if let Some(pinned) = pinned {
        let same = crate::home::user::sanitize_account_id(pinned)
            .map(|want| want == account)
            .unwrap_or(false);
        if !same {
            return Err(LoginError::Backend(format!(
                "MEDULLA_USER pins account {pinned}, but this token belongs to {user_id} — \
                 storing it would authenticate the pinned account as somebody else; \
                 unset MEDULLA_USER or sign in as {pinned}"
            )));
        }
    }

    // The account home this session belongs to, derived directly rather than
    // through `medulla_home` — which reads the marker, and the marker has not
    // moved yet. That ordering is the point: publishing first and writing after
    // meant a failed write (a full disk, a read-only mount) left the marker
    // pointing at a new, sessionless home while the previous account's working
    // session sat untouched on disk. A *failed* login would hide a good one.
    //
    // So the session is written into its own account's home first, and the
    // marker is published only once that has succeeded.
    let home = if pinned.is_none() {
        // Known safe: sanitized above, and its failure already returned.
        root.join(&account)
    } else {
        // An operator override pins the account, so the resolved home is
        // already the right one and the marker is not moving.
        crate::home::medulla_home(env)
    };
    std::fs::create_dir_all(&home).map_err(LoginError::Io)?;

    let record = StoredSession {
        token: jwt.to_string(),
        user_id: Some(user_id.clone()),
        base_url: base_url.trim_end_matches('/').to_string(),
    };
    let body = serde_json::to_vec_pretty(&record)
        .map_err(|e| LoginError::Backend(format!("could not encode session: {e}")))?;
    write_private(&location(&home), &body).map_err(LoginError::Io)?;

    // Published last, now that the session it points at is durable. A failure
    // above leaves the previous account selected and its session intact, which
    // is the state an operator can retry from.
    if pinned.is_none() {
        crate::home::user::write_active_user_id(&root, &user_id).map_err(LoginError::Io)?;
    }

    tracing::debug!("[auth] app session stored");
    Ok(AuthState {
        is_authenticated: true,
        user_id: Some(user_id),
    })
}

/// The stored session token, if there is one.
///
/// `Ok(None)` is the signed-out state, not a failure. Every reader that needs
/// the JWT itself takes it from here rather than reading a file of its own, so
/// there is exactly one place a session lives.
pub fn token(env: &HashMap<String, String>) -> Option<String> {
    read(env).map(|s| s.token).filter(|t| !t.trim().is_empty())
}

/// The full stored session record, if one is readable.
///
/// A malformed or unreadable file reads as signed out rather than as an error:
/// the only useful recovery is to sign in again, and failing a command over a
/// corrupt cache would block exactly that.
pub fn read(env: &HashMap<String, String>) -> Option<StoredSession> {
    let path = location(&crate::home::medulla_home(env));
    let body = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<StoredSession>(&body) {
        Ok(session) => Some(session),
        Err(err) => {
            tracing::debug!("[auth] ignoring unreadable session at {path:?}: {err}");
            None
        }
    }
}

/// Who this install is signed in as, for the Account surface.
pub fn state(env: &HashMap<String, String>) -> AuthState {
    match read(env) {
        Some(s) if !s.token.trim().is_empty() => AuthState {
            is_authenticated: true,
            user_id: s.user_id,
        },
        _ => AuthState::default(),
    }
}

/// Forget the stored session.
///
/// Idempotent: clearing when already signed out succeeds, reporting `Ok(None)`.
/// `Ok(Some(path))` is a file actually removed, so `logout` can say what it did.
///
/// # Errors
///
/// The removal's own error when a session exists and cannot be deleted — a
/// read-only mount, a permissions change, a Windows handle still open. This is
/// the case that must not be folded into "nothing to clear": logging out is a
/// security action, and reporting success while the bearer is still on disk
/// tells an operator they have revoked access they still hold.
pub fn clear(env: &HashMap<String, String>) -> std::io::Result<Option<PathBuf>> {
    let path = location(&crate::home::medulla_home(env));
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::debug!("[auth] app session cleared");
            Ok(Some(path))
        }
        // Already gone is the idempotent case, not a failure.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Write `body` to `path` with owner-only permissions.
///
/// The mode is set on the handle before the bytes are written, not with a
/// `set_permissions` call afterwards: the gap between a world-readable create
/// and a later chmod is exactly long enough for another process on the machine
/// to read the token out.
///
/// `OpenOptionsExt::mode` applies to a *newly created* file only, so a re-store
/// over a path that already exists inherits whatever mode it had — including one
/// an earlier build, a restored backup, or a stray `chmod` left world-readable.
/// Every write must therefore be a create.
///
/// # Why a temporary file rather than removing the destination
///
/// Removing first also satisfied the mode requirement, and cost the previous
/// session to do it: a create or write that failed afterwards — a full disk, an
/// I/O error, the process dying between the two — left the operator with no
/// session at all, or a half-written one. A failed re-store would have signed
/// them out.
///
/// The record is written to a sibling temporary, synced, and only then renamed
/// over the destination. `rename(2)` is atomic within a directory, so a reader
/// sees either the old file or the complete new one, and a failure anywhere
/// before the rename leaves the old session untouched.
pub fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the session path has no parent directory"))?;
    // Unique per *write*, not per process. A pid-scoped name collides with
    // itself: two turns saving the same resumed thread concurrently would both
    // pick it, and the loser can unlink the winner's open temporary and
    // recreate it — after which the winner renames a half-written file into
    // place. The counter closes that, and the pid keeps a leftover traceable to
    // the process that made it.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".to_string()),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));

    // No pre-emptive removal: the name is unique to this write, so anything
    // already there was not ours and `create_new` below is right to refuse it.

    let mut opts = std::fs::OpenOptions::new();
    // `create_new`, not `create`: anything already at this path was put there by
    // something else, and writing a bearer token into a file another process
    // created is precisely what the mode exists to prevent.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let write = (|| -> std::io::Result<()> {
        let mut file = opts.open(&temp)?;
        file.write_all(body)?;
        // Before the rename, not after: a rename that lands ahead of the data
        // can leave an empty file where a session used to be.
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();

    if write.is_err() {
        // The destination still holds the previous session, which is the whole
        // point of writing beside it. Clean up so the next attempt is not
        // tripped by our own leftover.
        let _ = std::fs::remove_file(&temp);
    }
    write
}

/// Adopt a credential left by a pre-cutover install, when this host is signed
/// out.
///
/// An install that predates the store holds a real bearer for the deployment
/// this store now serves, so it *is* signed in and should stay signed in. Each
/// candidate is verified through [`store`] like any other token; one the backend
/// rejects is left alone, and its file is removed only once the new store holds
/// a working session.
///
/// A no-op when this host is already authenticated — the existing credential
/// wins, and the stale file is swept rather than re-verified.
///
/// "Already authenticated" means the whole precedence chain, not just the store.
/// An inline `backend.token` or a `backend.tokenEnv` variable outranks the
/// session, so adopting under one would store a credential the runtime is never
/// going to present — and `store` moves the active-account marker, so a legacy
/// token belonging to a different account would re-home the install while the
/// runtime kept authenticating as somebody else. The account shown in the UI and
/// the identity on the wire would disagree, which is worse than not adopting.
///
/// Returns the paths actually removed, so a caller can say what it did.
pub async fn adopt_legacy(
    env: &HashMap<String, String>,
    backend: &crate::config::BackendConfig,
) -> Vec<PathBuf> {
    let base_url = backend.base_url.as_str();
    let already_signed_in =
        crate::auth::resolve_backend_token(env, backend, token(env).as_deref()).is_some();
    let home = crate::home::medulla_home(env);
    let mut removed = Vec::new();

    let mut adopted = already_signed_in;
    for legacy in crate::auth::adopt_legacy_credentials(env, &home) {
        // Only the first credential that verifies is adopted. Both historical
        // locations can hold a valid token, and continuing would store the
        // second over the first — silently preferring the OS config directory's
        // older copy to the account home's newer one.
        if !adopted {
            // The recorded origin, not the caller's: a credential written for a
            // different deployment must be verified against the one that issued
            // it, or a staging token is offered to production and rejected for
            // reasons that have nothing to do with whether it is still valid.
            let against = if legacy.base_url.trim().is_empty() {
                base_url
            } else {
                legacy.base_url.as_str()
            };
            match store(env, against, &legacy.jwt).await {
                Ok(_) => {
                    tracing::debug!("[auth] adopted a pre-cutover credential");
                    adopted = true;
                }
                Err(err) => {
                    // Left on disk deliberately. Removing a token this host
                    // could not verify would destroy the operator's only copy
                    // over what may be a transient backend failure.
                    tracing::debug!("[auth] a pre-cutover credential was not adopted: {err}");
                    continue;
                }
            }
        }
        if crate::auth::discard_legacy_credential(&legacy.path) {
            removed.push(legacy.path);
        }
    }
    removed
}
