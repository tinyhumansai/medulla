//! Generate a `prepare-commit-msg` git hook that appends a `Co-authored-by`
//! trailer from the `MEDULLA_ATTRIBUTION` environment variable. Used for
//! providers whose CLI has no built-in attribution knob (Codex, Opencode).
//!
//! On Unix the hook is a shell script placed in a temporary directory; the
//! caller activates it by exporting `GIT_CONFIG_KEY_0=core.hooksPath` and
//! `GIT_CONFIG_VALUE_0=<tmpdir>` alongside `GIT_CONFIG_COUNT=1`. On non-Unix
//! platforms this returns an empty map — git hooks are not supported there.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Environment key carrying the `Co-authored-by` trailer line for the hook to
/// read at runtime.
const MEDULLA_ATTRIBUTION_KEY: &str = "MEDULLA_ATTRIBUTION";

/// Generate a `prepare-commit-msg` hook script and return the environment
/// variables needed to activate it, plus the hook directory path for later
/// cleanup.
///
/// `trailer` is the full `Co-authored-by` line to append. The hook is a no-op
/// when the `MEDULLA_ATTRIBUTION` env var is empty or unset.
#[cfg(unix)]
pub fn generate_hook(trailer: &str) -> (HashMap<String, String>, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let hook_dir = tempfile::tempdir().expect("tempdir for git hook").keep();
    let hook_path = hook_dir.join("prepare-commit-msg");

    let script = "#!/bin/sh\n\
         # Medulla attribution hook: appends Co-authored-by trailer.\n\
         if [ -n \"$MEDULLA_ATTRIBUTION\" ]; then\n\
             echo \"\" >> \"$1\"\n\
             echo \"$MEDULLA_ATTRIBUTION\" >> \"$1\"\n\
         fi\n"
        .to_string();

    let mut file = std::fs::File::create(&hook_path).expect("create hook script");
    file.write_all(script.as_bytes())
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
