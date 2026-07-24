//! Unit tests for [`super::attribution`]: trailer shape, the kill-switch
//! precedence matrix, and per-provider coverage.

use std::collections::HashMap;

use super::{
    attribution_args, attribution_enabled, attribution_trailer, ATTRIBUTION_EMAIL,
    ATTRIBUTION_ENV_KEY, ATTRIBUTION_NAME,
};
use crate::tinyplace::HarnessProvider;

/// An env map with `TINYPLACE_GIT_ATTRIBUTION` set to `value`.
fn env_with(value: &str) -> HashMap<String, String> {
    HashMap::from([(ATTRIBUTION_ENV_KEY.to_string(), value.to_string())])
}

#[test]
fn trailer_uses_the_medulla_identity() {
    assert_eq!(
        attribution_trailer(),
        "Co-authored-by: Medulla <medulla@tinyhumans.ai>"
    );
    assert_eq!(ATTRIBUTION_NAME, "Medulla");
    assert_eq!(ATTRIBUTION_EMAIL, "medulla@tinyhumans.ai");
}

#[test]
fn attribution_is_enabled_by_default() {
    assert!(attribution_enabled(&HashMap::new()));
}

#[test]
fn kill_switch_disables_attribution() {
    for raw in ["0", "false", "no", "off", "", "  "] {
        assert!(
            !attribution_enabled(&env_with(raw)),
            "expected {raw:?} to disable attribution"
        );
    }
}

#[test]
fn affirmative_values_keep_attribution_on() {
    for raw in ["1", "true", "yes", "on", "TRUE", " On "] {
        assert!(
            attribution_enabled(&env_with(raw)),
            "expected {raw:?} to enable attribution"
        );
    }
}

/// An unrecognised value should fail closed rather than silently attributing.
#[test]
fn unrecognised_values_fail_closed() {
    assert!(!attribution_enabled(&env_with("maybe")));
}

#[test]
fn claude_receives_inline_settings_carrying_the_trailer() {
    let args = attribution_args(HarnessProvider::Claude, &HashMap::new());
    assert_eq!(args.len(), 2, "expected a flag/value pair, got {args:?}");
    assert_eq!(args[0], "--settings");

    let parsed: serde_json::Value =
        serde_json::from_str(&args[1]).expect("settings payload must be valid JSON");
    assert_eq!(
        parsed["attribution"]["commit"],
        serde_json::Value::String(attribution_trailer()),
    );
}

/// The payload must carry *only* `attribution.commit`, so it layers over the
/// operator's own settings without clobbering unrelated keys.
#[test]
fn claude_settings_payload_is_minimal() {
    let args = attribution_args(HarnessProvider::Claude, &HashMap::new());
    let parsed: serde_json::Value = serde_json::from_str(&args[1]).unwrap();

    let top = parsed.as_object().expect("payload is a JSON object");
    assert_eq!(top.len(), 1, "unexpected top-level keys: {top:?}");
    let attribution = parsed["attribution"]
        .as_object()
        .expect("attribution is a JSON object");
    assert_eq!(attribution.len(), 1, "unexpected keys: {attribution:?}");
}

/// Codex hardcodes its own trailer and Opencode has no knob at all, so Medulla
/// leaves both alone rather than misattributing them.
#[test]
fn providers_without_a_knob_receive_no_args() {
    for provider in [HarnessProvider::Codex, HarnessProvider::Opencode] {
        assert!(
            attribution_args(provider, &HashMap::new()).is_empty(),
            "{provider:?} should receive no attribution args"
        );
    }
}

#[test]
fn kill_switch_suppresses_args_for_every_provider() {
    let env = env_with("0");
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        assert!(
            attribution_args(provider, &env).is_empty(),
            "{provider:?} should receive no args when disabled"
        );
    }
}

// ---------------------------------------------------------------------------
// prepare_commit_msg hook generator tests
// ---------------------------------------------------------------------------

/// On Unix, `generate_hook` returns env vars carrying the attribution trailer
/// and the `core.hooksPath` git-config overrides.
#[cfg(unix)]
#[test]
fn generate_hook_returns_env_vars() {
    let (env, _hook_dir) =
        super::prepare_commit_msg::generate_hook("Co-authored-by: Medulla <medulla@tinyhumans.ai>");
    assert_eq!(
        env.get("MEDULLA_ATTRIBUTION"),
        Some(&"Co-authored-by: Medulla <medulla@tinyhumans.ai>".to_string()),
    );
    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"1".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0"),
        Some(&"core.hooksPath".to_string())
    );
    assert!(
        env.contains_key("GIT_CONFIG_VALUE_0"),
        "hooksPath must be set"
    );
}

/// The hook script must be an executable file at the expected path.
#[cfg(unix)]
#[test]
fn hook_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;

    let (_env, hook_dir) = super::prepare_commit_msg::generate_hook("test");
    let hook_path = hook_dir.join("prepare-commit-msg");
    assert!(hook_path.exists(), "hook script must exist: {hook_path:?}");

    let metadata = std::fs::metadata(&hook_path).expect("hook metadata");
    let perms = metadata.permissions();
    assert_eq!(perms.mode() & 0o111, 0o111, "hook must be executable");
}

/// The hook script appends a blank line followed by the trailer to the commit
/// message file when `MEDULLA_ATTRIBUTION` is set.
#[cfg(unix)]
#[test]
fn hook_script_appends_trailer() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let (_env, hook_dir) = super::prepare_commit_msg::generate_hook(trailer);

    // Simulate what git does: create a temp commit message file with some
    // content, then run the hook against it with MEDULLA_ATTRIBUTION set.
    let msg_file = hook_dir.join("COMMIT_EDITMSG");
    let original = "summary line\n\nbody text\n";
    std::fs::write(&msg_file, original).unwrap();

    let hook_path = hook_dir.join("prepare-commit-msg");
    let output = std::process::Command::new("sh")
        .arg(&hook_path)
        .arg(&msg_file)
        .env("MEDULLA_ATTRIBUTION", trailer)
        .output()
        .expect("hook execution");

    assert!(
        output.status.success(),
        "hook exited non-zero: {:?}",
        output
    );

    let result = String::from_utf8_lossy(&std::fs::read(&msg_file).unwrap()).into_owned();
    assert!(result.contains(original), "original content preserved");
    assert!(
        result.ends_with(&format!("\n{trailer}\n")) || result.ends_with(&format!("\n{trailer}")),
        "trailer appended: {result:?}"
    );
}

/// When `MEDULLA_ATTRIBUTION` is empty, the hook must not modify the commit
/// message.
#[cfg(unix)]
#[test]
fn hook_is_noop_when_attribution_env_is_empty() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let (_env, hook_dir) = super::prepare_commit_msg::generate_hook(trailer);

    let msg_file = hook_dir.join("COMMIT_EDITMSG");
    let original = "just a message\n";
    std::fs::write(&msg_file, original).unwrap();

    let hook_path = hook_dir.join("prepare-commit-msg");
    let output = std::process::Command::new("sh")
        .arg(&hook_path)
        .arg(&msg_file)
        .env_remove("MEDULLA_ATTRIBUTION")
        .output()
        .expect("hook execution");

    assert!(output.status.success());
    let result = String::from_utf8_lossy(&std::fs::read(&msg_file).unwrap()).into_owned();
    assert_eq!(result, original, "message unchanged");
}

/// Cleanup must remove the hook directory and its contents.
#[cfg(unix)]
#[test]
fn cleanup_removes_hook_dir() {
    let (_env, hook_dir) = super::prepare_commit_msg::generate_hook("test");
    assert!(hook_dir.exists(), "hook dir exists before cleanup");

    super::prepare_commit_msg::cleanup_hook_dir(&hook_dir);
    assert!(!hook_dir.exists(), "hook dir removed after cleanup");
}

/// On non-Unix, the generator returns an empty env map and no cleanup path.
#[cfg(not(unix))]
#[test]
fn non_unix_returns_empty() {
    let (env, hook_dir) = super::prepare_commit_msg::generate_hook("test");
    assert!(env.is_empty(), "non-Unix returns empty env");
    assert!(
        hook_dir.as_os_str().is_empty(),
        "non-Unix returns empty path"
    );
}
