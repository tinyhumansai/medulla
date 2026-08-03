//! Focused tests for recognizing stable worktree helper reports.

use super::{is_pull_request_command, pull_request_url, text_report};

#[test]
fn text_report_ignores_fields_before_its_ready_marker() {
    let output = concat!(
        "path: /unrelated\nbranch: stale\n",
        "[PASS] WORKTREE_READY\n",
        "  path: /repo/worktrees/fix-label\n",
        "  branch: fix-label\n",
        "  head: abc1234\n",
        "later output"
    );

    assert_eq!(
        text_report(output),
        Some((
            "/repo/worktrees/fix-label".to_string(),
            "fix-label".to_string()
        ))
    );
}

#[test]
fn github_pr_urls_are_normalized_from_plain_and_json_output() {
    assert_eq!(
        pull_request_url("https://github.com/tinyhumansai/medulla/pull/157\n"),
        Some("https://github.com/tinyhumansai/medulla/pull/157".to_string())
    );
    assert_eq!(
        pull_request_url(r#"{"url":"https://github.com/tinyhumansai/medulla/pull/158"}"#),
        Some("https://github.com/tinyhumansai/medulla/pull/158".to_string())
    );
}

#[test]
fn issues_and_malformed_pr_urls_are_not_session_context() {
    assert_eq!(
        pull_request_url("https://github.com/tinyhumansai/medulla/issues/157"),
        None
    );
    assert_eq!(
        pull_request_url("https://github.com/tinyhumansai/medulla/pull/latest"),
        None
    );
}

#[test]
fn only_direct_github_pr_commands_may_report_a_pull_request() {
    assert!(is_pull_request_command("gh pr create --fill"));
    assert!(is_pull_request_command("  gh pr view --json url"));
    assert!(!is_pull_request_command("rg 'gh pr view' fixtures"));
    assert!(!is_pull_request_command(
        "cat <<'EOF'\ngh pr view\nhttps://github.com/acme/other/pull/7\nEOF"
    ));
}
