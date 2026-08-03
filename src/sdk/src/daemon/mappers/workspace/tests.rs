//! Focused tests for recognizing stable worktree helper reports.

use super::{pull_request_command, pull_request_url, pull_request_url_from_json, text_report};

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
    assert!(pull_request_command("gh pr create --fill").is_some());
    assert!(pull_request_command("  gh pr view --json url").is_some());
    assert!(pull_request_command("/bin/zsh -lc 'gh pr view --json url'").is_some());
    assert!(pull_request_command("gh pr view 123 --repo other/project").is_none());
    assert!(pull_request_command("gh pr view feature-branch").is_none());
    assert!(pull_request_command("gh pr view https://github.com/acme/other/pull/7").is_none());
    assert!(pull_request_command("gh pr create --repo other/project --fill").is_none());
    assert!(pull_request_command("gh pr view --comments").is_none());
    assert!(pull_request_command("gh pr create --head other-branch").is_none());
    assert!(pull_request_command("gh pr create -Hother-branch").is_none());
    assert!(pull_request_command("rg 'gh pr view' fixtures").is_none());
    assert!(pull_request_command("/bin/zsh -lc 'cat <<EOF\ngh pr view\nEOF'").is_none());
    assert!(pull_request_command(
        "cat <<'EOF'\ngh pr view\nhttps://github.com/acme/other/pull/7\nEOF"
    )
    .is_none());
}

#[test]
fn structured_view_output_reads_only_the_url_property() {
    let output = r#"{
        "url":"https://github.com/acme/repo/pull/42",
        "body":"see https://github.com/acme/other/pull/7"
    }"#;
    assert_eq!(
        pull_request_url_from_json(output).as_deref(),
        Some("https://github.com/acme/repo/pull/42")
    );
    assert_eq!(
        pull_request_url_from_json(r#"{"body":"https://github.com/acme/other/pull/7"}"#),
        None
    );
}
