//! Checkout identity behavior when Git metadata cannot host a marker.

use std::process::Command;

#[test]
fn read_only_git_metadata_uses_a_validatable_fallback_identity() {
    let directory = tempfile::tempdir().expect("repository");
    assert!(Command::new("git")
        .current_dir(directory.path())
        .args(["init", "--quiet"])
        .status()
        .expect("git init")
        .success());
    let identity = super::super::checkout::capture_with(directory.path(), |_, _| false)
        .expect("fallback identity");
    let matches = super::super::checkout::matches(directory.path(), &identity);

    assert!(identity.starts_with("metadata:"), "{identity}");
    assert!(matches);
}
