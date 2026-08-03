//! Focused tests for recognizing stable worktree helper reports.

use super::{
    pull_request_command as recognize_pull_request_command, pull_request_url,
    pull_request_url_from_json, text_report, workspace_event_from_output, PullRequestCommand,
};

/// Recognize commands with deterministic, explicitly clear `GH_REPO` state.
fn pull_request_command(command: &str, workspace_cwd: Option<&str>) -> Option<PullRequestCommand> {
    recognize_pull_request_command(command, workspace_cwd, false)
}

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
    assert_eq!(
        pull_request_url("https://github.com/acme/pull/pull/42"),
        Some("https://github.com/acme/pull/pull/42".to_string())
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
    assert!(pull_request_command("gh pr create --fill", None).is_some());
    assert!(pull_request_command("gh pr create --title 'R&D cleanup'", None).is_some());
    assert!(pull_request_command("  gh pr view --json url", None).is_some());
    assert!(pull_request_command("/bin/zsh -lc 'gh pr view --json url'", None).is_some());
    assert!(pull_request_command("gh pr view 123 --repo other/project", None).is_none());
    assert!(pull_request_command("gh pr view feature-branch", None).is_none());
    assert!(
        pull_request_command("gh pr view https://github.com/acme/other/pull/7", None).is_none()
    );
    assert!(pull_request_command("gh pr create --repo other/project --fill", None).is_none());
    assert!(pull_request_command("gh pr view --comments", None).is_none());
    assert!(pull_request_command("gh pr view --json url,title", None).is_none());
    assert!(pull_request_command("gh pr create --head other-branch", None).is_none());
    assert!(pull_request_command("gh pr create -Hother-branch", None).is_none());
    assert!(pull_request_command("gh pr create -dHother-branch", None).is_none());
    assert!(pull_request_command("gh pr create -dRother/project", None).is_none());
    assert!(pull_request_command("gh pr create -tHotfix", None).is_some());
    assert!(pull_request_command("gh pr create '--head' other-branch", None).is_none());
    assert!(pull_request_command("gh pr create --hea\\d other-branch", None).is_none());
    assert!(pull_request_command(
        "gh pr create --dry-run --body 'see https://github.com/acme/other/pull/7'",
        None
    )
    .is_none());
    assert!(pull_request_command(
        "gh pr create --dry-run=true --body 'see https://github.com/acme/other/pull/7'",
        None
    )
    .is_none());
    assert!(pull_request_command("rg 'gh pr view' fixtures", None).is_none());
    assert!(pull_request_command("/bin/zsh -lc 'cat <<EOF\ngh pr view\nEOF'", None).is_none());
    assert!(pull_request_command(
        "cat <<'EOF'\ngh pr view\nhttps://github.com/acme/other/pull/7\nEOF",
        None
    )
    .is_none());
}

#[test]
fn inherited_github_repository_override_disqualifies_pr_commands() {
    assert!(recognize_pull_request_command("gh pr create --fill", None, true).is_none());
    assert!(recognize_pull_request_command("gh pr view --json url", None, true).is_none());
}

#[test]
fn pr_commands_reject_chains_except_for_the_reported_worktree_cd() {
    let cwd = "/repo/worktrees/fix-label";
    assert!(pull_request_command(
        "gh pr create --fill >/dev/null && cat fixtures/pr_urls.txt",
        None
    )
    .is_none());
    assert!(pull_request_command("gh pr create --fill; cat fixture", None).is_none());
    assert!(pull_request_command(
        "gh pr create --fill $(cat fixtures/pr_urls.txt >/dev/stderr) >/dev/null",
        None
    )
    .is_none());
    assert!(pull_request_command("gh pr create --body `cat fixture`", None).is_none());
    assert!(pull_request_command("cd /other && gh pr create --fill", Some(cwd)).is_none());
    assert!(pull_request_command("gh pr create --fill", Some(cwd)).is_none());
    assert!(pull_request_command(
        "cd /repo/w; cat /tmp/pr-url && gh pr create --fill",
        Some("/repo/w; cat /tmp/pr-url")
    )
    .is_none());
    assert!(pull_request_command(
        "cd '/repo/w; cat /tmp/pr-url' && gh pr create --fill",
        Some("/repo/w; cat /tmp/pr-url")
    )
    .is_some());
    assert!(pull_request_command("zsh -lc 'gh pr create --fill' ; cat 'x'", None).is_none());
    assert!(pull_request_command(
        "cd /repo/worktrees/fix-label && gh pr create --fill",
        Some(cwd)
    )
    .is_some());
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

#[test]
fn workspace_events_compose_checkout_and_pull_request_fields() {
    let checkout = "[PASS] WORKTREE_READY\n  path: /repo/worktrees/fix\n  branch: fix\n";
    let checkout_only = workspace_event_from_output(checkout, None, 1, 2, "test").unwrap();
    assert_eq!(checkout_only.event.payload["cwd"], "/repo/worktrees/fix");
    assert_eq!(checkout_only.event.payload["branch"], "fix");
    assert!(checkout_only.event.payload.get("pull_request").is_none());

    let url = "https://github.com/acme/repo/pull/42";
    let pr_only =
        workspace_event_from_output(url, Some(PullRequestCommand::Create), 1, 2, "test").unwrap();
    assert_eq!(pr_only.event.payload["pull_request"], url);
    assert!(pr_only.event.payload.get("branch").is_none());

    let combined = workspace_event_from_output(
        &format!("{checkout}{url}\n"),
        Some(PullRequestCommand::Create),
        1,
        2,
        "test",
    )
    .unwrap();
    assert_eq!(combined.event.payload["cwd"], "/repo/worktrees/fix");
    assert_eq!(combined.event.payload["pull_request"], url);
}
