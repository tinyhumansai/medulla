//! What [`super::enable`] is and is not allowed to change in Codex's config.

use std::collections::HashSet;

use super::*;

/// The events a Medulla spawn typically installs, as the caller passes them.
const INSTALLED: [HookEvent; 3] = [
    HookEvent::PostToolUse,
    HookEvent::SessionStart,
    HookEvent::SubagentStop,
];

fn events() -> HashSet<String> {
    INSTALLED.iter().map(|event| state_event(*event)).collect()
}

/// One `[hooks.state]` table, as Codex writes them.
fn entry(key: &str, body: &str) -> String {
    format!("[hooks.state.\"{key}\"]\ntrusted_hash = \"sha256:abc\"\n{body}\n")
}

fn injected(event: &str) -> String {
    format!("{INJECTED_SOURCE}:{event}:0:0")
}

#[test]
fn codex_spells_our_events_in_snake_case() {
    assert_eq!(state_event(HookEvent::PostToolUse), "post_tool_use");
    assert_eq!(state_event(HookEvent::Stop), "stop");
    assert_eq!(
        state_event(HookEvent::UserPromptSubmit),
        "user_prompt_submit"
    );
}

/// The shape Codex writes when it first registers a hook: a hash and no flag at
/// all. Nothing flips it but us, so this is the case that matters most.
#[test]
fn a_freshly_registered_entry_gains_the_flag() {
    let text = entry(&injected("post_tool_use"), "");
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 1);
    assert!(out.contains("enabled = true"), "{out}");
    assert!(out.contains("trusted_hash = \"sha256:abc\""), "{out}");
}

#[test]
fn an_explicitly_disabled_entry_is_flipped_in_place() {
    let text = entry(&injected("session_start"), "enabled = false");
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 1);
    assert!(out.contains("enabled = true"), "{out}");
    assert!(!out.contains("enabled = false"), "{out}");
}

#[test]
fn an_already_enabled_entry_is_left_alone() {
    let text = entry(&injected("post_tool_use"), "enabled = true");
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 0);
    assert_eq!(out, text);
}

/// The whole point of the source filter: a hook the workspace or the operator
/// wrote is not Medulla's to approve, however plausible its key looks.
#[test]
fn hooks_from_other_sources_are_never_touched() {
    let mut text = entry("/home/me/.codex/hooks.json:post_tool_use:0:0", "");
    text.push_str(&entry("/repo/.codex/hooks.json:post_tool_use:0:0", ""));
    text.push_str(&entry("plugin:acme:post_tool_use:0:0", ""));
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 0);
    assert_eq!(out, text);
}

/// An injected entry for an event this spawn is not installing belongs to some
/// other hook set — approving it would re-authorize a hook the operator has
/// since removed from their config.
#[test]
fn injected_entries_for_uninstalled_events_are_left_alone() {
    let text = entry(&injected("pre_tool_use"), "");
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 0);
    assert_eq!(out, text);
}

/// Everything around the edit is Codex's file, and it has to come back byte for
/// byte — an operator's `[projects]` trust levels and model settings included.
#[test]
fn unrelated_configuration_survives_verbatim() {
    let text = format!(
        "model = \"gpt-5.6-terra\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n\n{}\n[tui]\nresume_cwd = \"session\"\n",
        entry(&injected("post_tool_use"), "")
    );
    let (out, changed) = rewrite(&text, &events());
    assert_eq!(changed, 1);
    for line in [
        "model = \"gpt-5.6-terra\"",
        "[projects.\"/work\"]",
        "trust_level = \"trusted\"",
        "[tui]",
        "resume_cwd = \"session\"",
    ] {
        assert!(out.contains(line), "{line} must survive: {out}");
    }
}

/// The appended flag has to land inside its own table, not after the blank line
/// that separates it from the next one — where it would silently become a key
/// of whatever follows.
#[test]
fn the_appended_flag_stays_inside_its_own_table() {
    let text = format!(
        "{}\n[tui]\nresume_cwd = \"session\"\n",
        entry(&injected("post_tool_use"), "")
    );
    let (out, _) = rewrite(&text, &events());
    let flag = out.find("enabled = true").expect("the flag was written");
    let next = out.find("[tui]").expect("the next table survives");
    assert!(flag < next, "the flag must precede the next table: {out}");
}

/// Rewriting twice must be a no-op — this runs on every Codex spawn.
#[test]
fn enabling_is_idempotent() {
    let text = entry(&injected("subagent_stop"), "");
    let (once, first) = rewrite(&text, &events());
    let (twice, second) = rewrite(&once, &events());
    assert_eq!((first, second), (1, 0));
    assert_eq!(once, twice);
}

/// A key that is injected-shaped but malformed is not ours to guess at.
#[test]
fn malformed_keys_are_not_claimed() {
    let ours = events();
    assert!(!is_ours(INJECTED_SOURCE, &ours));
    assert!(!is_ours(&format!("{INJECTED_SOURCE}:post_tool_use"), &ours));
    assert!(!is_ours(
        &format!("{INJECTED_SOURCE}:post_tool_use:0"),
        &ours
    ));
    assert!(!is_ours(
        &format!("{INJECTED_SOURCE}:post_tool_use:a:0"),
        &ours
    ));
    assert!(!is_ours(
        &format!("{INJECTED_SOURCE}:post_tool_use:0:0:0"),
        &ours
    ));
    assert!(is_ours(
        &format!("{INJECTED_SOURCE}:post_tool_use:0:1"),
        &ours
    ));
}

/// A machine where Codex has never run is the ordinary first-launch case, not
/// an error to fail a spawn over.
#[test]
fn a_missing_config_is_not_an_error() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let missing = dir.path().join("config.toml");
    assert_eq!(enable(&missing, &INSTALLED).expect("no error"), 0);
    assert!(!missing.exists(), "nothing is created for Codex");
}

/// End to end on a real file, including the atomic replacement.
#[test]
fn enable_writes_the_file_and_reports_the_count() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("config.toml");
    let text = format!(
        "{}{}",
        entry(&injected("post_tool_use"), ""),
        entry(&injected("session_start"), "enabled = false")
    );
    std::fs::write(&path, &text).expect("the config is writable");

    assert_eq!(enable(&path, &INSTALLED).expect("no error"), 2);
    let after = std::fs::read_to_string(&path).expect("readable");
    assert_eq!(after.matches("enabled = true").count(), 2, "{after}");
    assert_eq!(enable(&path, &INSTALLED).expect("no error"), 0);
    assert!(
        dir.path().join("config.toml.medulla-trust.lock").exists(),
        "the stable sibling lock coordinates subsequent writers too"
    );
}

#[cfg(unix)]
#[test]
fn enable_preserves_a_symlink_and_restrictive_permissions() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let dir = tempfile::tempdir().expect("a scratch directory");
    let target = dir.path().join("dotfiles-config.toml");
    let link = dir.path().join("config.toml");
    std::fs::write(&target, entry(&injected("post_tool_use"), "")).expect("the target is writable");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
        .expect("the target permissions can be restricted");
    symlink(&target, &link).expect("a symlinked configuration");

    assert_eq!(enable(&link, &INSTALLED).expect("rewrite succeeds"), 1);
    assert!(std::fs::symlink_metadata(&link)
        .expect("link metadata")
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::metadata(&target)
            .expect("target metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(std::fs::read_to_string(&target)
        .expect("target contents")
        .contains("enabled = true"));
}
