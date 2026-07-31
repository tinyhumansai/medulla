//! Generate the git hook directory that adds a `Co-authored-by` trailer from
//! the `MEDULLA_ATTRIBUTION` environment variable.
//!
//! This is the deterministic attribution path, used for every provider. A
//! harness CLI's own attribution knob (where one exists at all) is advisory —
//! it only asks the *model* to write the trailer, so it silently drops out
//! whenever the task brief dictates a commit message. The hook runs inside
//! `git commit` itself and cannot be talked out of it.
//!
//! On Unix the hooks live in a temporary directory; the caller activates them by
//! exporting `GIT_CONFIG_KEY_0=core.hooksPath` and `GIT_CONFIG_VALUE_0=<dir>`
//! alongside `GIT_CONFIG_COUNT=1`. On non-Unix platforms this returns an empty
//! map — git hooks are not supported there.
//!
//! # Why the whole hook set is shimmed
//!
//! `core.hooksPath` redirects *every* hook, not just `prepare-commit-msg`. A
//! directory holding only that one hook would therefore silently disable the
//! repository's `pre-commit`, `commit-msg`, `pre-push` and friends — turning off
//! exactly the lint and validation gates a repo relies on. So the directory
//! holds a shim under every client-side hook name; each shim resolves the
//! repository's real hooks directory and delegates to the hook of the same name
//! (propagating its exit code, so a failing `pre-commit` still blocks the
//! commit), then adds the attribution behaviour that belongs to
//! `prepare-commit-msg` alone.
//!
//! # Why the directory is process-wide
//!
//! The shim reads the trailer from the environment at run time, so its contents
//! are identical for every spawn — there is nothing per-run to isolate. One
//! directory per process, created once and never removed while the process
//! lives, is what makes concurrent harnesses safe: a per-spawn directory would
//! let one task's cleanup delete the directory a concurrently running task's
//! child still has `core.hooksPath` pointing at, silently disabling its
//! attribution (and, with the shims above, its repository's own hooks too).
//!
//! The cost is one small temp directory per process, reclaimed by the OS.

use std::collections::HashMap;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::OnceLock;

/// Environment key carrying the `Co-authored-by` trailer line for the hook to
/// read at runtime.
const MEDULLA_ATTRIBUTION_KEY: &str = "MEDULLA_ATTRIBUTION";

/// Environment key telling the shim how many `GIT_CONFIG_*` pairs came from the
/// parent, so it can drop only the one we appended when it resolves the
/// repository's real hooks directory.
const BASE_COUNT_KEY: &str = "MEDULLA_GIT_CONFIG_BASE_COUNT";

/// Client-side hooks git may invoke. Every one gets a shim so that redirecting
/// `core.hooksPath` does not disable the repository's own copy.
///
/// Server-side hooks (`pre-receive`, `update`, `post-receive`, `proc-receive`)
/// are omitted: they run in the receiving repository, never in a harness's
/// working clone.
#[cfg(unix)]
const CLIENT_HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "post-rewrite",
    "pre-auto-gc",
    "push-to-checkout",
    "reference-transaction",
    "sendemail-validate",
    "post-index-change",
];

/// The shim script. One copy is written under each name in [`CLIENT_HOOKS`] and
/// branches on `basename "$0"`, so all hooks share a single implementation.
#[cfg(unix)]
const HOOK_SHIM: &str = r#"#!/bin/sh
# Medulla hook shim. Runs the repository's own hook of this name, then adds the
# Medulla Co-authored-by trailer (prepare-commit-msg only).

hook_name=$(basename "$0")
self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# `core.hooksPath` points at us, so resolve the repo's real hooks directory with
# the injected git config suppressed, and run its hook first — it must see the
# arguments git handed us, not ones we have already edited. Its exit code is
# ours: a failing pre-commit or commit-msg must still block the commit.
# Drop only the entry we appended, keeping any GIT_CONFIG_* the parent set --
# a CI-supplied core.hooksPath is one we must still delegate to. --path expands
# `~/...`, which git does natively but `git config --get` does not.
base_count=${MEDULLA_GIT_CONFIG_BASE_COUNT:-0}
orig_dir=$(GIT_CONFIG_COUNT=$base_count git config --path --get core.hooksPath 2>/dev/null)
if [ -z "$orig_dir" ]; then
    orig_dir=$(GIT_CONFIG_COUNT=$base_count git rev-parse --git-path hooks 2>/dev/null)
fi
if [ -n "$orig_dir" ] && [ "$orig_dir" != "$self_dir" ] && [ -x "$orig_dir/$hook_name" ]; then
    "$orig_dir/$hook_name" "$@" || exit $?
fi

# Everything below is the attribution behaviour, which belongs to one hook.
[ "$hook_name" = "prepare-commit-msg" ] || exit 0
[ -n "$MEDULLA_ATTRIBUTION" ] || exit 0

# Merge and squash messages are composed by git over commits the harness did not
# author; a trailer there would credit Medulla for someone else's work.
case "$2" in
    merge|squash) exit 0 ;;
esac

# addIfDifferent makes this idempotent across amend/rebase/cherry-pick, and
# interpret-trailers appends into the existing trailer block instead of opening
# a new one. Never fail the commit over attribution.
git interpret-trailers --in-place \
    --if-exists addIfDifferent \
    --trailer "$MEDULLA_ATTRIBUTION" \
    "$1" 2>/dev/null || true

exit 0
"#;

/// The process-wide hook directory, created on first use.
///
/// Deliberately never removed while the process lives — see the module docs on
/// why a per-spawn directory is unsafe under concurrency.
#[cfg(unix)]
static HOOK_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Environment variables that activate the attribution hook for a child
/// process, creating the shared hook directory on first call.
///
/// `trailer` is the full `Co-authored-by` line to add. The hook is a no-op when
/// `MEDULLA_ATTRIBUTION` is empty or unset, so clearing that single variable
/// disables attribution without unwinding the git-config injection.
///
/// Safe to call concurrently: every caller receives the same directory, whose
/// contents do not vary per run.
///
/// `base_env` is the environment these entries will be merged into. Any
/// `GIT_CONFIG_*` pairs already there are preserved and ours is *appended* at
/// the next index: `GIT_CONFIG_COUNT` is the number of pairs git will read, so
/// resetting it to `1` would silently discard whatever the parent set — a CI
/// supplying `user.email`, `safe.directory`, or its own `core.hooksPath`.
///
/// Returns an empty map if the hook directory could not be created — attribution
/// is a nicety, and failing to write a temp file must never stop a harness from
/// running.
#[cfg(unix)]
pub fn hook_env(trailer: &str, base_env: &HashMap<String, String>) -> HashMap<String, String> {
    let Some(hook_dir) = HOOK_DIR.get_or_init(|| build_hook_dir().ok()).as_ref() else {
        return HashMap::new();
    };

    // A malformed inherited count is treated as absent: appending after a value
    // git itself would reject helps nobody.
    let base_count: usize = base_env
        .get("GIT_CONFIG_COUNT")
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(0);

    let mut env = HashMap::new();
    env.insert(MEDULLA_ATTRIBUTION_KEY.to_string(), trailer.to_string());
    env.insert(BASE_COUNT_KEY.to_string(), base_count.to_string());
    env.insert("GIT_CONFIG_COUNT".to_string(), (base_count + 1).to_string());
    env.insert(
        format!("GIT_CONFIG_KEY_{base_count}"),
        "core.hooksPath".to_string(),
    );
    env.insert(
        format!("GIT_CONFIG_VALUE_{base_count}"),
        hook_dir.to_string_lossy().into_owned(),
    );
    env
}

#[cfg(not(unix))]
pub fn hook_env(_trailer: &str, _base_env: &HashMap<String, String>) -> HashMap<String, String> {
    HashMap::new()
}

/// Write the shim under every client-side hook name in a fresh temp directory.
#[cfg(unix)]
fn build_hook_dir() -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let hook_dir = tempfile::tempdir()?.keep();
    for name in CLIENT_HOOKS {
        let path = hook_dir.join(name);
        let mut file = std::fs::File::create(&path)?;
        file.write_all(HOOK_SHIM.as_bytes())?;
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(hook_dir)
}

/// Build a standalone hook directory, for tests that need one they own rather
/// than the process-wide one [`hook_env`] shares.
#[cfg(all(unix, test))]
pub fn generate_hook_dir() -> PathBuf {
    build_hook_dir().expect("build hook dir")
}

/// Remove a hook directory, ignoring errors (best-effort cleanup).
pub fn cleanup_hook_dir(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::remove_dir_all(path);
}
