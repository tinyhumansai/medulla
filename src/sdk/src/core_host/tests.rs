//! Unit tests for the settings this host resolves before the core is built.
//!
//! These cover the resolution only, not [`super::boot`] — booting a core touches
//! process globals (`OnceLock` context, singleton event bus) and cannot be torn
//! down between tests. Boot coverage belongs in an integration test with one
//! core per test binary.
//!
//! # No environment lock any more
//!
//! These tests used to serialize on a mutex and clear four process environment
//! variables around every case, because the functions under test wrote to the
//! process environment. They do not: resolution takes an env *map* and returns a
//! value, so each case is independent and the suite runs in parallel. That the
//! lock could be deleted is the point of the change, not a side effect of it.

use super::*;

#[test]
fn the_backend_api_url_is_resolved_so_auth_me_hits_the_configured_deployment() {
    // The staging failure this exists for: OpenHuman resolves `/auth/me` from
    // its own config chain and falls back to production, so a staging token
    // verified by the login flow was then handed to production to validate —
    // and rejected.
    assert_eq!(
        resolve_backend_api_url(&HashMap::new(), "https://staging-api.tinyhumans.ai/"),
        "https://staging-api.tinyhumans.ai"
    );
}

#[test]
fn an_operator_backend_override_wins_in_either_spelling() {
    for key in [OPENHUMAN_BACKEND_URL_ENV, OPENHUMAN_BACKEND_URL_ALT_ENV] {
        let env = HashMap::from([(key.to_string(), "https://self.hosted".to_string())]);
        assert_eq!(
            resolve_backend_api_url(&env, "https://api.tinyhumans.ai"),
            "https://self.hosted",
            "{key} should have won"
        );
    }
}

#[test]
fn a_blank_backend_url_resolves_to_nothing() {
    // Empty means "let the core resolve its own", which is different from
    // pointing it at the empty string.
    assert_eq!(resolve_backend_api_url(&HashMap::new(), "   "), "");
}

#[test]
fn a_blank_operator_override_does_not_shadow_the_config() {
    let env = HashMap::from([(OPENHUMAN_BACKEND_URL_ENV.to_string(), "  ".to_string())]);
    assert_eq!(
        resolve_backend_api_url(&env, "https://api.tinyhumans.ai"),
        "https://api.tinyhumans.ai"
    );
}

#[test]
fn the_workspace_derives_from_medulla_home_when_unset() {
    // The scratch-run recipe: `MEDULLA_HOME=$(mktemp -d)` must isolate the
    // core's state too, or memory/flows/credentials land in the developer's
    // real `~/.openhuman`.
    assert_eq!(
        resolve_workspace(&HashMap::new(), Path::new("/tmp/scratch-home")),
        workspace_dir(Path::new("/tmp/scratch-home"))
    );
}

#[test]
fn the_workspace_keeps_an_explicit_operator_override() {
    // What lets a developer aim the embedded core at an existing OpenHuman
    // install on purpose.
    let env = HashMap::from([(
        OPENHUMAN_WORKSPACE_ENV.to_string(),
        "/opt/openhuman/ws".to_string(),
    )]);
    assert_eq!(
        resolve_workspace(&env, Path::new("/tmp/scratch-home")),
        PathBuf::from("/opt/openhuman/ws")
    );
}

#[test]
fn a_blank_workspace_override_counts_as_unset() {
    let env = HashMap::from([(OPENHUMAN_WORKSPACE_ENV.to_string(), "   ".to_string())]);
    assert_eq!(
        resolve_workspace(&env, Path::new("/tmp/scratch-home")),
        workspace_dir(Path::new("/tmp/scratch-home"))
    );
}

#[test]
fn the_action_dir_comes_from_the_configured_workspace_root() {
    assert_eq!(
        resolve_action_dir(&HashMap::new(), Some(Path::new("/repos/work"))),
        Some(PathBuf::from("/repos/work"))
    );
}

#[test]
fn no_workspace_root_means_no_action_dir() {
    // Rather than binding something arbitrary, which would aim the agent's
    // write root at a directory this host has never used.
    assert_eq!(resolve_action_dir(&HashMap::new(), None), None);
}

#[test]
fn the_action_dir_keeps_an_operator_override() {
    let env = HashMap::from([(
        OPENHUMAN_ACTION_DIR_ENV.to_string(),
        "/opt/projects".to_string(),
    )]);
    assert_eq!(
        resolve_action_dir(&env, Some(Path::new("/repos/work"))),
        Some(PathBuf::from("/opt/projects"))
    );
}

#[test]
fn resolve_covers_everything_a_lazy_boot_needs() {
    // The lazy boot path (`shared`) has no caller of its own, so a host that
    // loaded its layered config publishes these instead. They must reproduce
    // the TUI's startup settings: workspace from MEDULLA_HOME, action dir from
    // the first configured workspace root, backend from `backend.baseUrl`.
    let mut config = crate::config::TuiConfig::default();
    config.backend.base_url = "https://staging-api.tinyhumans.ai/".to_string();
    config.workflow.workspaces = vec!["/repos/work".to_string()];

    let settings = CoreSettings::resolve(&HashMap::new(), &config, Path::new("/tmp/scratch-home"));

    assert_eq!(
        settings.workspace,
        workspace_dir(Path::new("/tmp/scratch-home"))
    );
    assert_eq!(settings.action_dir, Some(PathBuf::from("/repos/work")));
    // One value now covers what took two bindings: with no explicit
    // `OPENHUMAN_MEDULLA_BASE_URL`, the core's Medulla client falls through to
    // the same `api_url` this sets.
    assert_eq!(settings.backend_url, "https://staging-api.tinyhumans.ai");
}

#[test]
fn resolve_leaves_the_operators_own_choices_alone() {
    let mut config = crate::config::TuiConfig::default();
    config.backend.base_url = "https://api.tinyhumans.ai".to_string();
    config.workflow.workspaces = vec!["/repos/work".to_string()];
    let env = HashMap::from([
        (
            OPENHUMAN_WORKSPACE_ENV.to_string(),
            "/opt/openhuman/ws".to_string(),
        ),
        (
            OPENHUMAN_BACKEND_URL_ENV.to_string(),
            "https://self.hosted".to_string(),
        ),
    ]);

    let settings = CoreSettings::resolve(&env, &config, Path::new("/tmp/scratch-home"));

    assert_eq!(settings.workspace, PathBuf::from("/opt/openhuman/ws"));
    assert_eq!(settings.backend_url, "https://self.hosted");
    // Not overridden, so still derived from the config.
    assert_eq!(settings.action_dir, Some(PathBuf::from("/repos/work")));
}

#[test]
fn the_floor_is_workspace_isolation_and_nothing_else() {
    // What a core booted with no config at all gets. Deliberately not "nothing":
    // without the workspace it would write into the developer's real
    // `~/.openhuman`.
    let settings = CoreSettings::floor(&HashMap::new(), Path::new("/tmp/scratch-home"));
    assert_eq!(
        settings.workspace,
        workspace_dir(Path::new("/tmp/scratch-home"))
    );
    assert_eq!(settings.action_dir, None);
    assert!(settings.backend_url.is_empty());
}

#[test]
fn the_workspace_directory_name_is_load_bearing() {
    // OpenHuman derives its *config* directory from the workspace path's
    // parent, so a directory literally called `workspace` puts config at
    // `<medulla_home>/config.toml` beside it rather than inside the state tree.
    assert_eq!(
        workspace_dir(Path::new("/tmp/scratch-home")),
        PathBuf::from("/tmp/scratch-home/workspace")
    );
}
