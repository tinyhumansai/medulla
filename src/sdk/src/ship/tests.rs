//! Offline parsing and fake-binary coverage for the ship client.

use std::fs;

use super::client::parse_remote_slug;
use super::parse::{parse_prs, parse_unresolved_threads};
use super::*;

#[test]
fn parses_green_failing_and_pending_check_rollups() {
    let rows = parse_prs(
        r#"[
          {"number":1,"title":"green","headRefName":"a","url":"https://x/1",
           "statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}]},
          {"number":2,"title":"red","headRefName":"b","url":"https://x/2",
           "statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]},
          {"number":3,"title":"wait","headRefName":"c","url":"https://x/3",
           "statusCheckRollup":[{"status":"IN_PROGRESS","conclusion":null}]},
          {"number":4,"title":"none","headRefName":"d","url":"https://x/4",
           "statusCheckRollup":[]}
        ]"#,
    )
    .unwrap();
    assert_eq!(rows[0].checks, CheckState::Green);
    assert_eq!(rows[1].checks, CheckState::Failing);
    assert_eq!(rows[2].checks, CheckState::Pending);
    assert_eq!(rows[3].checks, CheckState::Pending);
    assert_eq!(CheckState::Green.label(), "green");
    assert_eq!(CheckState::Failing.label(), "failing");
    assert_eq!(CheckState::Pending.label(), "pending");
}

#[test]
fn counts_only_unresolved_review_threads() {
    let fixture = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
      {"isResolved":false},{"isResolved":true},{"isResolved":false}
    ]}}}}}"#;
    assert_eq!(parse_unresolved_threads(fixture).unwrap(), 2);
}

#[test]
fn remote_parser_accepts_canonical_github_shapes() {
    assert_eq!(
        parse_remote_slug("git@github.com:tinyhumansai/medulla.git").as_deref(),
        Some("tinyhumansai/medulla")
    );
    assert_eq!(
        parse_remote_slug("https://github.com/tinyhumansai/medulla").as_deref(),
        Some("tinyhumansai/medulla")
    );
    assert!(parse_remote_slug("https://example.com/owner/repo").is_none());
}

#[cfg(unix)]
#[test]
fn fake_gh_covers_reads_actions_absence_and_auth_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("gh");
    fs::write(
        &fake,
        r#"#!/bin/sh
printf '%s\n' "$*" >> gh.calls
case "$1 $2" in
  "auth status") if [ -f .unauth ]; then echo "not logged in" >&2; exit 1; fi ;;
  "repo view") echo "tinyhumansai/medulla" ;;
  "pr list") echo '[{"number":7,"title":"Ship it","headRefName":"feat/x","url":"https://example/pr/7","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}]}]' ;;
  "api graphql") echo '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[{"isResolved":false}]}}}}}' ;;
  "pr checks") echo '[{"bucket":"fail","link":"https://github.com/tinyhumansai/medulla/actions/runs/9","name":"ci"}]' ;;
  "run view") printf 'setup\ncompile failed\nassertion failed\n' ;;
  "pr create") echo "https://github.com/tinyhumansai/medulla/pull/99" ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "upstream",
            "git@github.com:tinyhumansai/medulla.git",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let client = ShipClient::with_binary(&fake);
    let reports = client.inspect_workspaces(&[dir.path().to_path_buf()]);
    let ShipState::Ready(rows) = &reports[0].state else {
        panic!("fake gh should be available: {:?}", reports[0].state);
    };
    assert_eq!(rows[0].number, 7);
    assert_eq!(rows[0].unresolved_threads, 1);
    assert!(client
        .failing_log_excerpt(dir.path(), 7)
        .unwrap()
        .contains("assertion failed"));
    client.open_pr(dir.path(), 7).unwrap();
    assert!(client.create_pr(dir.path()).unwrap().ends_with("/pull/99"));

    fs::write(dir.path().join(".unauth"), "").unwrap();
    assert!(matches!(
        client.inspect_workspace(dir.path()),
        ShipState::GhUnavailable(reason) if reason.contains("not logged in")
    ));
    let missing = ShipClient::with_binary(dir.path().join("missing"));
    assert!(matches!(
        missing.inspect_workspace(dir.path()),
        ShipState::GhUnavailable(reason) if reason.contains("gh unavailable")
    ));
}
