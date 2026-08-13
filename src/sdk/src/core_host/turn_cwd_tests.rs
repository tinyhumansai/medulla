//! Unit tests for the per-turn working-directory task-local: that a scope is
//! visible inside it, does not leak out of it, and that a caller with nothing to
//! declare installs nothing.

use std::path::{Path, PathBuf};

use super::turn_cwd::{current_turn_cwd, with_turn_cwd};

#[tokio::test]
async fn outside_a_scope_there_is_no_turn_directory() {
    assert_eq!(current_turn_cwd(), None);
}

#[tokio::test]
async fn a_scope_is_visible_to_everything_awaited_inside_it() {
    let observed = with_turn_cwd(Some(Path::new("/repo/worktrees/feature")), async {
        current_turn_cwd()
    })
    .await;
    assert_eq!(observed, Some(PathBuf::from("/repo/worktrees/feature")));
}

#[tokio::test]
async fn a_scope_does_not_leak_past_the_turn_that_opened_it() {
    with_turn_cwd(Some(Path::new("/repo/one")), async {
        assert_eq!(current_turn_cwd(), Some(PathBuf::from("/repo/one")));
    })
    .await;
    assert_eq!(current_turn_cwd(), None);
}

#[tokio::test]
async fn nested_scopes_report_the_innermost_turn() {
    with_turn_cwd(Some(Path::new("/repo/outer")), async {
        let inner =
            with_turn_cwd(Some(Path::new("/repo/inner")), async { current_turn_cwd() }).await;
        assert_eq!(inner, Some(PathBuf::from("/repo/inner")));
        assert_eq!(current_turn_cwd(), Some(PathBuf::from("/repo/outer")));
    })
    .await;
}

#[tokio::test]
async fn an_absent_or_blank_directory_installs_nothing() {
    let none = with_turn_cwd(None, async { current_turn_cwd() }).await;
    assert_eq!(none, None, "a caller with no directory declares none");
    let blank = with_turn_cwd(Some(Path::new("")), async { current_turn_cwd() }).await;
    assert_eq!(blank, None, "an empty path is not a directory to report");
}
