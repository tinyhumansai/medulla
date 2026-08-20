//! Unit tests for deriving a checkout identity from raw Git output.

use super::*;

#[test]
fn a_primary_checkout_has_no_worktree_name() {
    let checkout = derive(
        ".git",
        ".git",
        "/w/medulla-public",
        Some("main"),
        Some("a1b2c3d"),
    );
    assert_eq!(checkout.repo.as_deref(), Some("medulla-public"));
    assert_eq!(checkout.worktree, None);
    assert_eq!(checkout.branch.as_deref(), Some("main"));
    assert!(!checkout.is_worktree());
    assert!(checkout.is_repository());
}

#[test]
fn a_linked_worktree_is_named_and_keeps_the_repository_name() {
    let checkout = derive(
        "/w/medulla-public/.git/worktrees/feature-x",
        "/w/medulla-public/.git",
        "/w/worktrees/feature-x/medulla-public",
        Some("feature-x"),
        Some("a1b2c3d"),
    );
    // The repository name comes from the common directory, so it is the one
    // every worktree of it shares rather than this checkout's own folder.
    assert_eq!(checkout.repo.as_deref(), Some("medulla-public"));
    assert_eq!(checkout.worktree.as_deref(), Some("feature-x"));
    assert!(checkout.is_worktree());
}

#[test]
fn a_relative_worktree_git_dir_is_still_recognized() {
    let checkout = derive(
        "../../.git/worktrees/feature-x",
        "../../.git",
        "/w/worktrees/feature-x",
        Some("feature-x"),
        None,
    );
    assert_eq!(checkout.worktree.as_deref(), Some("feature-x"));
    // No parent component to read a name from, so the checkout root answers.
    assert_eq!(checkout.repo.as_deref(), Some("feature-x"));
}

#[test]
fn a_bare_repository_drops_the_git_suffix() {
    let checkout = derive(
        "/w/medulla.git",
        "/w/medulla.git",
        "/w/medulla",
        Some("main"),
        None,
    );
    assert_eq!(checkout.repo.as_deref(), Some("medulla"));
}

#[test]
fn a_directory_outside_git_is_not_a_repository() {
    let checkout = derive("", "", "", None, None);
    assert!(!checkout.is_repository());
    assert_eq!(checkout.branch_label(), None);
    assert_eq!(checkout.summary(), None);
}

#[test]
fn a_detached_head_is_labelled_by_its_commit() {
    let checkout = derive(".git", ".git", "/w/medulla", None, Some("a1b2c3d"));
    assert_eq!(checkout.branch, None);
    assert_eq!(checkout.branch_label().as_deref(), Some("@a1b2c3d"));
}

#[test]
fn an_unborn_branch_has_a_name_but_no_commit() {
    let checkout = derive(".git", ".git", "/w/fresh", Some("main"), None);
    assert_eq!(checkout.branch_label().as_deref(), Some("main"));
    assert_eq!(checkout.head, None);
}

#[test]
fn blank_git_answers_are_read_as_absent() {
    let checkout = derive(".git", ".git", "/w/medulla", Some("  "), Some(""));
    assert_eq!(checkout.branch, None);
    assert_eq!(checkout.head, None);
    assert_eq!(checkout.branch_label(), None);
}

#[test]
fn a_summary_names_the_repository_worktree_and_branch() {
    let checkout = derive(
        "/w/medulla/.git/worktrees/feature-x",
        "/w/medulla/.git",
        "/w/worktrees/feature-x",
        Some("feature-x"),
        Some("a1b2c3d"),
    );
    assert_eq!(
        checkout.summary().as_deref(),
        Some("medulla ⑂ feature-x · feature-x")
    );
}

#[test]
fn a_primary_checkout_summary_omits_the_worktree() {
    let checkout = derive(".git", ".git", "/w/medulla", Some("main"), Some("a1b2c3d"));
    assert_eq!(checkout.summary().as_deref(), Some("medulla · main"));
}
