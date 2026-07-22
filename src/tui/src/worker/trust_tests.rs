//! Tests for [`super::trust`] — pre-trusting the workspace with Claude Code.
//!
//! Every test writes to a temp config. Nothing here reads or touches a real
//! `~/.claude.json`: the whole point of this module is that it edits somebody
//! else's file, so its tests must not.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use super::trust::{
    config_path, ensure_bypass_accepted, ensure_workspace_trusted, grant, is_trusted,
    settings_path, TrustOutcome,
};

/// An env pointing claude's config at `dir`.
fn env_at(dir: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        dir.to_string_lossy().into_owned(),
    );
    env
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_config_dir_override_wins_over_home() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/x".to_string());
    assert_eq!(
        config_path(&env).unwrap(),
        Path::new("/home/x/.claude.json")
    );

    env.insert("CLAUDE_CONFIG_DIR".to_string(), "/cfg".to_string());
    assert_eq!(config_path(&env).unwrap(), Path::new("/cfg/.claude.json"));

    assert_eq!(config_path(&HashMap::new()), None, "no home, no guess");
}

#[test]
fn trust_is_inherited_from_an_ancestor() {
    // Claude walks up the tree, so trusting a parent covers everything beneath
    // it. Writing a redundant entry for each child would be noise in a file we
    // do not own.
    let config = json!({"projects": {"/work": {"hasTrustDialogAccepted": true}}});
    assert!(is_trusted(&config, Path::new("/work")));
    assert!(is_trusted(&config, Path::new("/work/repo/sub")));
    assert!(!is_trusted(&config, Path::new("/elsewhere")));
    assert!(
        !is_trusted(&config, Path::new("/workshop")),
        "a path prefix is not an ancestor"
    );
}

#[test]
fn an_untrusted_or_absent_entry_is_not_trust() {
    let config = json!({"projects": {"/work": {"hasTrustDialogAccepted": false}}});
    assert!(!is_trusted(&config, Path::new("/work")));
    assert!(!is_trusted(&json!({}), Path::new("/work")));
    assert!(!is_trusted(&json!({"projects": null}), Path::new("/work")));
}

#[test]
fn granting_preserves_everything_else_in_the_entry() {
    // This is somebody else's config. Losing their allowed tools or MCP servers
    // to fix a dialog would be a far worse bug than the one being fixed.
    let mut config = json!({
        "projects": {
            "/work": {"allowedTools": ["Bash(git *)"], "hasCompletedProjectOnboarding": true}
        }
    });
    grant(&mut config, Path::new("/work"));
    let entry = &config["projects"]["/work"];
    assert_eq!(entry["hasTrustDialogAccepted"], json!(true));
    assert_eq!(entry["allowedTools"], json!(["Bash(git *)"]));
    assert_eq!(entry["hasCompletedProjectOnboarding"], json!(true));
}

#[test]
fn ensure_writes_the_flag_and_keeps_unrelated_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    // Keys this code has never heard of must survive the round trip.
    std::fs::write(
        &path,
        json!({
            "hasCompletedOnboarding": true,
            "someFutureKey": {"nested": [1, 2, 3]},
            "projects": {"/other": {"hasTrustDialogAccepted": true}}
        })
        .to_string(),
    )
    .unwrap();

    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let outcome = ensure_workspace_trusted(&env_at(dir.path()), workspace.to_str().unwrap());
    assert_eq!(outcome, TrustOutcome::Granted(path.clone()));

    let written = read(&path);
    let key = std::fs::canonicalize(&workspace)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        written["projects"][&key]["hasTrustDialogAccepted"],
        json!(true)
    );
    assert_eq!(written["hasCompletedOnboarding"], json!(true));
    assert_eq!(written["someFutureKey"]["nested"], json!([1, 2, 3]));
    assert_eq!(
        written["projects"]["/other"]["hasTrustDialogAccepted"],
        json!(true),
        "another project's trust must not be disturbed"
    );
}

#[test]
fn an_already_trusted_workspace_is_left_completely_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let key = std::fs::canonicalize(&workspace)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let original = json!({"projects": {key: {"hasTrustDialogAccepted": true}}}).to_string();
    std::fs::write(&path, &original).unwrap();

    let outcome = ensure_workspace_trusted(&env_at(dir.path()), workspace.to_str().unwrap());
    assert_eq!(outcome, TrustOutcome::AlreadyTrusted);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "not even reformatted — an untouched file is untouched"
    );
    assert_eq!(outcome.log_line("x"), None, "the normal case says nothing");
}

#[test]
fn a_missing_or_invalid_config_is_skipped_not_created() {
    // Claude onboards the operator itself; inventing a config it has not
    // written yet is not this module's business, and must never stop the worker
    // from starting.
    let dir = tempfile::tempdir().unwrap();
    let outcome = ensure_workspace_trusted(&env_at(dir.path()), dir.path().to_str().unwrap());
    assert!(matches!(outcome, TrustOutcome::Skipped(_)), "{outcome:?}");
    assert!(
        !dir.path().join(".claude.json").exists(),
        "no config may be conjured"
    );

    std::fs::write(dir.path().join(".claude.json"), "{ not json").unwrap();
    let outcome = ensure_workspace_trusted(&env_at(dir.path()), dir.path().to_str().unwrap());
    match outcome {
        TrustOutcome::Skipped(why) => assert!(why.contains("not valid JSON"), "{why}"),
        other => panic!("unparseable config must be left alone, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".claude.json")).unwrap(),
        "{ not json",
        "an unreadable config must not be overwritten"
    );
}

#[test]
fn the_log_line_names_the_workspace_and_the_file() {
    let line = TrustOutcome::Granted("/cfg/.claude.json".into())
        .log_line("/work")
        .expect("granting is worth saying out loud");
    assert!(line.contains("/work"), "{line}");
    assert!(line.contains("/cfg/.claude.json"), "{line}");
}

#[test]
fn the_bypass_disclaimer_is_accepted_in_the_settings_file() {
    // Verified against the installed CLI: the *config* key claude once used for
    // this is migrated away and no longer suppresses the disclaimer — only the
    // settings key does. They are different files, so this must not drift back
    // into `config_path`.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        settings_path(&env_at(dir.path())).unwrap(),
        dir.path().join("settings.json"),
        "settings live beside the config, not in it"
    );

    let outcome = ensure_bypass_accepted(&env_at(dir.path()));
    let path = dir.path().join("settings.json");
    assert_eq!(outcome, TrustOutcome::Granted(path.clone()));
    assert_eq!(
        read(&path)["skipDangerousModePermissionPrompt"],
        json!(true)
    );

    // Idempotent: a second launch writes nothing and says nothing.
    let again = ensure_bypass_accepted(&env_at(dir.path()));
    assert_eq!(again, TrustOutcome::AlreadyTrusted);
    assert_eq!(again.log_line("accepted it"), None);
}

#[test]
fn accepting_the_disclaimer_preserves_existing_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        json!({"model": "opus", "permissions": {"allow": ["Bash(git *)"]}}).to_string(),
    )
    .unwrap();

    assert!(matches!(
        ensure_bypass_accepted(&env_at(dir.path())),
        TrustOutcome::Granted(_)
    ));
    let written = read(&path);
    assert_eq!(written["skipDangerousModePermissionPrompt"], json!(true));
    assert_eq!(written["model"], json!("opus"));
    assert_eq!(written["permissions"]["allow"], json!(["Bash(git *)"]));
}

#[test]
fn the_default_settings_path_is_under_dot_claude() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/x".to_string());
    assert_eq!(
        settings_path(&env).unwrap(),
        Path::new("/home/x/.claude/settings.json")
    );
}

#[test]
fn a_skip_says_out_loud_what_it_could_not_do() {
    // The already-settled case is silent, but a skip is the one outcome an
    // operator needs to see, so it must render a line naming the failure.
    let line = TrustOutcome::Skipped("no HOME or CLAUDE_CONFIG_DIR".to_string())
        .log_line("trust /work")
        .expect("a skip is worth logging");
    assert!(line.contains("could not trust /work"), "{line}");
    assert!(line.contains("no HOME or CLAUDE_CONFIG_DIR"), "{line}");
}

#[test]
fn without_a_home_or_config_dir_both_preflights_skip_rather_than_guess() {
    // With nowhere to look, guessing at a path would write the flag where
    // nothing reads it — so both steps decline rather than invent a location.
    assert!(matches!(
        ensure_bypass_accepted(&HashMap::new()),
        TrustOutcome::Skipped(_)
    ));
    assert!(matches!(
        ensure_workspace_trusted(&HashMap::new(), "/work"),
        TrustOutcome::Skipped(_)
    ));
}

#[test]
fn an_unparseable_settings_file_is_left_untouched() {
    // The settings file is the operator's; a value this code cannot parse is
    // never overwritten, only skipped.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{ not json").unwrap();
    match ensure_bypass_accepted(&env_at(dir.path())) {
        TrustOutcome::Skipped(why) => assert!(why.contains("not valid JSON"), "{why}"),
        other => panic!("an unparseable settings file must be skipped, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ not json",
        "the operator's file must not be rewritten"
    );
}

#[test]
fn a_settings_file_that_is_not_an_object_is_replaced_before_the_flag_is_set() {
    // A settings file holding a non-object (an array, say) must not make
    // inserting the key panic — it is replaced with an object carrying the flag.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "[1, 2, 3]").unwrap();
    assert!(matches!(
        ensure_bypass_accepted(&env_at(dir.path())),
        TrustOutcome::Granted(_)
    ));
    assert_eq!(
        read(&path)["skipDangerousModePermissionPrompt"],
        json!(true)
    );
}

#[test]
fn bypass_is_skipped_when_its_directory_cannot_be_created() {
    // A config dir *underneath a regular file* can never be created, so the
    // write is declined rather than crashing the launch.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        blocker.join("sub").to_string_lossy().into_owned(),
    );
    match ensure_bypass_accepted(&env) {
        TrustOutcome::Skipped(why) => assert!(why.contains("could not create"), "{why}"),
        other => panic!("an uncreatable dir must be skipped, got {other:?}"),
    }
}

#[test]
fn bypass_is_skipped_when_the_settings_file_cannot_be_written() {
    // A directory sitting where the settings file belongs makes the atomic
    // rename fail; the failure is reported, not panicked on.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("settings.json")).unwrap();
    match ensure_bypass_accepted(&env_at(dir.path())) {
        TrustOutcome::Skipped(why) => assert!(why.contains("could not write"), "{why}"),
        other => panic!("an unwritable settings file must be skipped, got {other:?}"),
    }
}

#[test]
fn granting_repairs_unexpected_shapes_at_every_level() {
    // The config is claude's; a value of the wrong shape at any level — the root,
    // `projects`, or a single entry — is replaced rather than panicked on, and
    // the flag still lands.
    let mut root_wrong = json!([1, 2, 3]);
    grant(&mut root_wrong, Path::new("/work"));
    assert_eq!(
        root_wrong["projects"]["/work"]["hasTrustDialogAccepted"],
        json!(true)
    );

    let mut projects_wrong = json!({"projects": 5});
    grant(&mut projects_wrong, Path::new("/work"));
    assert_eq!(
        projects_wrong["projects"]["/work"]["hasTrustDialogAccepted"],
        json!(true)
    );

    let mut entry_wrong = json!({"projects": {"/work": "not-an-object"}});
    grant(&mut entry_wrong, Path::new("/work"));
    assert_eq!(
        entry_wrong["projects"]["/work"]["hasTrustDialogAccepted"],
        json!(true)
    );
}

#[test]
fn workspace_trust_is_skipped_when_the_config_cannot_be_written() {
    // The config parses fine and the workspace is untrusted, but the atomic
    // write's temp path is blocked by a directory of the same name — a write
    // failure must skip, never truncate the operator's config.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    std::fs::write(&path, json!({"projects": {}}).to_string()).unwrap();
    std::fs::create_dir(dir.path().join(".claude.json.medulla-tmp")).unwrap();

    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    match ensure_workspace_trusted(&env_at(dir.path()), workspace.to_str().unwrap()) {
        TrustOutcome::Skipped(why) => assert!(why.contains("could not write"), "{why}"),
        other => panic!("an unwritable config must be skipped, got {other:?}"),
    }
    assert_eq!(
        read(&path)["projects"],
        json!({}),
        "the original config must survive a failed write"
    );
}
