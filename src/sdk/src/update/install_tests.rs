//! Tests for the install module.

//! Inline tests for the module-private filesystem helpers, which the sibling
//! `update::tests` module cannot reach.
use super::*;

#[test]
fn make_workdir_creates_a_fresh_dir() {
    let dir = make_workdir().unwrap();
    assert!(dir.exists() && dir.is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_binary_locates_nested_and_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    // An empty tree has no binary.
    assert!(find_binary(dir.path()).is_none());
    // Place the platform binary a couple of levels deep, alongside a decoy.
    let nested = dir.path().join("pkg").join("bin");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.path().join("README"), b"x").unwrap();
    let bin = nested.join(bin_name());
    std::fs::write(&bin, b"binary").unwrap();
    assert_eq!(find_binary(dir.path()), Some(bin));
}

#[test]
fn move_file_moves_within_a_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    std::fs::write(&from, b"payload").unwrap();
    move_file(&from, &to).unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"payload");
    assert!(!from.exists(), "source is removed after the move");
}

#[cfg(unix)]
#[test]
fn set_executable_sets_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("bin");
    std::fs::write(&f, b"x").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
    set_executable(&f).unwrap();
    let mode = std::fs::metadata(&f).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn install_binary_without_existing_target() {
    // When there is no target yet, no backup is created and the new binary
    // simply lands in place.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("medulla");
    let staged = dir.path().join("staged");
    std::fs::write(&staged, b"NEW").unwrap();
    install_binary(&staged, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert!(!backup_path(&target).exists());
}

#[test]
fn extract_archive_unpacks_a_tarball() {
    // Build a tarball with `tar` and round-trip it through extract_archive.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hi").unwrap();
    let archive = dir.path().join("asset.tar.gz");
    let ok = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg("hello.txt")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return; // `tar` unavailable — skip rather than fail.
    }
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    extract_archive(&archive, &dest, false).unwrap();
    assert_eq!(std::fs::read(dest.join("hello.txt")).unwrap(), b"hi");
}

#[cfg(not(windows))]
#[test]
fn extract_archive_unpacks_a_zip() {
    // Build a zip with the `zip` CLI and round-trip it through the unzip path.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("payload.txt");
    std::fs::write(&src, b"zipped").unwrap();
    let archive = dir.path().join("asset.zip");
    let ok = std::process::Command::new("zip")
        .arg("-j")
        .arg(&archive)
        .arg(&src)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return; // `zip` unavailable — skip rather than fail.
    }
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    extract_archive(&archive, &dest, true).unwrap();
    assert_eq!(std::fs::read(dest.join("payload.txt")).unwrap(), b"zipped");
}
