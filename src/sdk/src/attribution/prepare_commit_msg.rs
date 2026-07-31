//! Generate a `prepare-commit-msg` git hook that appends a `Co-authored-by`
//! trailer from the `MEDULLA_ATTRIBUTION` environment variable.
//!
//! This is the deterministic attribution path, used for every provider. A
//! harness CLI's own attribution knob (where one exists at all) is advisory —
//! it only asks the *model* to write the trailer, so it silently drops out
//! whenever the task brief dictates a commit message. The hook runs inside
//! `git commit` itself and cannot be talked out of it.
//!
//! On Unix the hook is a shell script placed in a temporary directory; the
//! caller activates it by exporting `GIT_CONFIG_KEY_0=core.hooksPath` and
//! `GIT_CONFIG_VALUE_0=<tmpdir>` alongside `GIT_CONFIG_COUNT=1`. On non-Unix
//! platforms this returns an empty map — git hooks are not supported there.
//!
//! # Why the script is not a one-line append
//!
//! `core.hooksPath` redirects *every* hook, not just ours, so the script chains
//! to whatever `prepare-commit-msg` the repository already had — otherwise
//! pointing git at our temp directory would silently disable the repo's own
//! hook. The trailer itself is applied with `git interpret-trailers`, which
//! places it in the existing trailer block rather than starting a second one
//! (a blank line between trailers hides the earlier ones from GitHub) and
//! which, with `--if-exists addIfDifferent`, is idempotent — an amend, rebase,
//! or cherry-pick replays a message that already carries the trailer, and a
//! second copy is not a second co-author.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Environment key carrying the `Co-authored-by` trailer line for the hook to
/// read at runtime.
const MEDULLA_ATTRIBUTION_KEY: &str = "MEDULLA_ATTRIBUTION";

/// The `prepare-commit-msg` script body. See the module docs for why each step
/// is here.
#[cfg(unix)]
const HOOK_SCRIPT: &str = r#"#!/bin/sh
# Medulla attribution hook: adds a Co-authored-by trailer to commits made by a
# Medulla-launched harness, then chains to the repository's own hook.

msg_file="$1"
msg_source="$2"

self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# `core.hooksPath` points at us, so resolve the repo's real hooks directory with
# the injected git config suppressed, and run its hook first — it must see the
# message git handed us, not one we have already edited.
orig_dir=$(GIT_CONFIG_COUNT=0 git config --get core.hooksPath 2>/dev/null)
if [ -z "$orig_dir" ]; then
    orig_dir=$(GIT_CONFIG_COUNT=0 git rev-parse --git-path hooks 2>/dev/null)
fi
if [ -n "$orig_dir" ] && [ "$orig_dir" != "$self_dir" ] && [ -x "$orig_dir/prepare-commit-msg" ]; then
    "$orig_dir/prepare-commit-msg" "$@" || exit $?
fi

[ -n "$MEDULLA_ATTRIBUTION" ] || exit 0

# Merge and squash messages are composed by git over commits the harness did not
# author; a trailer there would credit Medulla for someone else's work.
case "$msg_source" in
    merge|squash) exit 0 ;;
esac

# addIfDifferent makes this idempotent across amend/rebase/cherry-pick, and
# interpret-trailers appends into the existing trailer block instead of opening
# a new one. Never fail the commit over attribution.
git interpret-trailers --in-place \
    --if-exists addIfDifferent \
    --trailer "$MEDULLA_ATTRIBUTION" \
    "$msg_file" 2>/dev/null || true

exit 0
"#;

/// Generate a `prepare-commit-msg` hook script and return the environment
/// variables needed to activate it, plus the hook directory path for later
/// cleanup.
///
/// `trailer` is the full `Co-authored-by` line to add. The hook is a no-op when
/// the `MEDULLA_ATTRIBUTION` env var is empty or unset, so clearing that single
/// variable disables attribution without unwinding the git config injection.
#[cfg(unix)]
pub fn generate_hook(trailer: &str) -> (HashMap<String, String>, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let hook_dir = tempfile::tempdir().expect("tempdir for git hook").keep();
    let hook_path = hook_dir.join("prepare-commit-msg");

    let mut file = std::fs::File::create(&hook_path).expect("create hook script");
    file.write_all(HOOK_SCRIPT.as_bytes())
        .expect("write hook script");
    drop(file);

    // Make the hook executable.
    let mut perms = std::fs::metadata(&hook_path)
        .expect("hook metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&hook_path, perms).expect("set hook executable");

    let hook_dir_str = hook_dir.to_string_lossy().into_owned();

    let mut env = HashMap::new();
    env.insert(MEDULLA_ATTRIBUTION_KEY.to_string(), trailer.to_string());
    env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
    env.insert("GIT_CONFIG_KEY_0".to_string(), "core.hooksPath".to_string());
    env.insert("GIT_CONFIG_VALUE_0".to_string(), hook_dir_str);

    (env, hook_dir)
}

#[cfg(not(unix))]
pub fn generate_hook(_trailer: &str) -> (HashMap<String, String>, PathBuf) {
    (HashMap::new(), PathBuf::new())
}

/// Remove the temporary hook directory, ignoring errors (best-effort cleanup).
pub fn cleanup_hook_dir(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::remove_dir_all(path);
}
