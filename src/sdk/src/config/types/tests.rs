//! Unit tests for the config data-model types. Serde defaults, camelCase
//! parsing, and round-trip behaviour for each `[section]` the TUI reads.

use super::*;

#[test]
fn harness_favorite_workspaces_round_trip_with_their_names() {
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"harness":{"favoriteWorkspaces":[{"name":"Medulla","path":"/work/medulla"}]}}"#,
    )
    .unwrap();

    assert_eq!(cfg.harness.favorite_workspaces[0].name, "Medulla");
    assert_eq!(cfg.harness.favorite_workspaces[0].path, "/work/medulla");
    assert!(serde_json::to_string(&cfg)
        .unwrap()
        .contains("\"favoriteWorkspaces\""));
}
