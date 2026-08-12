//! Unit tests for resolving a run's workspace.

use std::collections::HashMap;
use std::path::PathBuf;

use super::resolve;

/// A session directory with a `nested/` child, for the relative cases.
fn session() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::create_dir(dir.path().join("nested")).expect("the nested dir");
    dir
}

fn env_with_home(home: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([("HOME".to_string(), home.to_string_lossy().to_string())])
}

#[test]
fn no_workspace_keeps_the_session_directory() {
    let dir = session();
    let resolved = resolve(None, dir.path(), &HashMap::new()).expect("the session directory");
    assert_eq!(resolved, dir.path());
}

#[test]
fn a_blank_workspace_is_treated_as_unset() {
    let dir = session();
    for blank in ["", "   "] {
        let resolved = resolve(Some(blank), dir.path(), &HashMap::new()).expect("no refusal");
        assert_eq!(resolved, dir.path(), "'{blank}' should read as unset");
    }
}

#[test]
fn a_relative_workspace_resolves_under_the_session_directory() {
    let dir = session();
    let resolved = resolve(Some("nested"), dir.path(), &HashMap::new()).expect("the nested dir");
    assert_eq!(
        resolved,
        dir.path().join("nested").canonicalize().expect("canonical")
    );
}

/// The point of the parameter: a directory the session's own root does not
/// contain, which is what the script policy refuses when spelled as `args.cwd`.
#[test]
fn an_absolute_workspace_outside_the_session_is_accepted() {
    let session = session();
    let elsewhere = tempfile::tempdir().expect("a second temp dir");
    let resolved = resolve(
        Some(&elsewhere.path().to_string_lossy()),
        session.path(),
        &HashMap::new(),
    )
    .expect("the other directory");
    assert_eq!(
        resolved,
        elsewhere.path().canonicalize().expect("canonical")
    );
}

/// `..` is likewise how a caller reaches a sibling checkout, and is resolved
/// rather than refused — the workspace *is* the boundary, so it has none.
#[test]
fn a_parent_traversal_is_resolved_rather_than_refused() {
    let dir = session();
    let resolved = resolve(Some("nested/.."), dir.path(), &HashMap::new()).expect("the parent");
    assert_eq!(resolved, dir.path().canonicalize().expect("canonical"));
}

#[test]
fn a_tilde_expands_from_home() {
    let home = session();
    let elsewhere = tempfile::tempdir().expect("a session dir");
    let env = env_with_home(home.path());

    let bare = resolve(Some("~"), elsewhere.path(), &env).expect("home itself");
    assert_eq!(bare, home.path().canonicalize().expect("canonical"));

    let child = resolve(Some("~/nested"), elsewhere.path(), &env).expect("a child of home");
    assert_eq!(
        child,
        home.path()
            .join("nested")
            .canonicalize()
            .expect("canonical")
    );
}

/// Without `HOME` there is nothing to expand to, so the path is left alone and
/// fails as the missing directory it is — rather than silently becoming the
/// session's own.
#[test]
fn a_tilde_without_home_is_refused_rather_than_ignored() {
    let dir = session();
    let error = resolve(Some("~/nested"), dir.path(), &HashMap::new()).expect_err("a refusal");
    assert!(
        error.to_string().contains("does not exist"),
        "unexpected: {error}"
    );
}

#[test]
fn a_missing_workspace_is_refused_by_name() {
    let dir = session();
    let error = resolve(Some("no-such-dir"), dir.path(), &HashMap::new()).expect_err("a refusal");
    let message = error.to_string();
    assert!(message.contains("no-such-dir"), "unexpected: {message}");
    assert!(message.contains("does not exist"), "unexpected: {message}");
}

#[test]
fn a_file_is_not_a_workspace() {
    let dir = session();
    std::fs::write(dir.path().join("VERSION"), "1").expect("the file");
    let error = resolve(Some("VERSION"), dir.path(), &HashMap::new()).expect_err("a refusal");
    assert!(
        error.to_string().contains("is not a directory"),
        "unexpected: {error}"
    );
}

/// A missing session directory is left exactly as it was passed: this is the
/// historical no-parameter path, and canonicalizing it would start failing runs
/// that work today.
#[test]
fn an_unset_workspace_is_not_checked() {
    let missing = PathBuf::from("/definitely/not/here");
    let resolved = resolve(None, &missing, &HashMap::new()).expect("no refusal");
    assert_eq!(resolved, missing);
}
