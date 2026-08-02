//! Focused tests for recognizing stable worktree helper reports.

use super::text_report;

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
