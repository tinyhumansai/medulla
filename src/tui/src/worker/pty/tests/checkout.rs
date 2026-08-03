//! Checkout identity behavior when Git metadata cannot host a marker.

use std::os::unix::fs::PermissionsExt;
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
    let git_dir = directory.path().join(".git");
    let original = git_dir.metadata().expect("metadata").permissions();
    std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o555))
        .expect("make read-only");

    let identity = super::super::checkout::capture(directory.path()).expect("fallback identity");
    let matches = super::super::checkout::matches(directory.path(), &identity);

    std::fs::set_permissions(&git_dir, original).expect("restore permissions");
    assert!(identity.starts_with("metadata:"), "{identity}");
    assert!(matches);
}
