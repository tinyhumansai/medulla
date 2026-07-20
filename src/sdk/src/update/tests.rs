//! Unit tests for update version comparison, manifest parsing, platform
//! selection, hashing, and the atomic binary install.

use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[test]
fn semver_compare_matrix() {
    // newer
    assert!(is_newer("1.2.3", "1.2.4"));
    assert!(is_newer("1.2.3", "1.3.0"));
    assert!(is_newer("1.2.3", "2.0.0"));
    // leading-v tolerance on both sides
    assert!(is_newer("v1.2.3", "v1.2.4"));
    assert!(is_newer("1.2.3", "v2.0.0"));
    // equal / older → not newer
    assert!(!is_newer("1.2.3", "1.2.3"));
    assert!(!is_newer("2.0.0", "1.9.9"));
    assert!(!is_newer("1.2.4", "1.2.3"));
    // pre-release suffix on the core triple is ignored
    assert!(!is_newer("1.2.3", "1.2.3-rc1"));
    // garbage on either side → never newer
    assert!(!is_newer("1.2.3", "not-a-version"));
    assert!(!is_newer("garbage", "1.2.3"));
    assert!(!is_newer("1.2", "1.2.0"));
    assert!(!is_newer("1.2.3", "1.2.3.4"));
}

#[test]
fn manifest_parse_and_platform_pick() {
    let json = r#"{
        "version": "3.9.0",
        "tag": "v3.9.0",
        "pubDate": "2026-07-18T00:00:00Z",
        "notes": "https://example/notes",
        "platforms": {
            "aarch64-apple-darwin": {"url": "https://example/a.tar.gz", "sha256": "aa"},
            "x86_64-pc-windows-msvc": {"url": "https://example/w.zip", "sha256": "bb"}
        }
    }"#;
    let m = parse_manifest(json).unwrap();
    assert_eq!(m.version, "3.9.0");
    assert_eq!(m.tag, "v3.9.0");
    assert_eq!(m.pub_date, "2026-07-18T00:00:00Z");
    assert_eq!(m.platforms.len(), 2);
    let entry = m.platforms.get("aarch64-apple-darwin").unwrap();
    assert_eq!(entry.url, "https://example/a.tar.gz");
    assert_eq!(entry.sha256, "aa");
}

#[test]
fn platform_key_is_a_known_triple_or_unknown() {
    let key = platform_key();
    let known = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "unknown",
    ];
    assert!(known.contains(&key), "unexpected platform key: {key}");
}

#[test]
fn pick_platform_absent_is_none() {
    let m = Manifest {
        version: "9.9.9".into(),
        tag: "v9.9.9".into(),
        pub_date: String::new(),
        notes: String::new(),
        platforms: HashMap::new(),
    };
    assert!(pick_platform(&m).is_none());
}

#[test]
fn sha256_matches_known_vector() {
    // SHA-256 of the empty string.
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    // SHA-256 of "abc".
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn backup_path_appends_old() {
    let bp = backup_path(Path::new("/usr/local/bin/medulla"));
    assert_eq!(bp, PathBuf::from("/usr/local/bin/medulla.old"));
}

#[test]
fn install_binary_replaces_and_backs_up() {
    let dir = std::env::temp_dir().join(format!(
        "medulla-install-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("medulla");
    std::fs::write(&target, b"OLD").unwrap();
    let new = dir.join("staged");
    std::fs::write(&new, b"NEW").unwrap();

    install_binary(&new, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(std::fs::read(backup_path(&target)).unwrap(), b"OLD");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh temp directory for one install test.
fn install_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-install-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn installing_where_nothing_exists_yet_needs_no_backup() {
    // A first install has no current binary to move aside; that must not be
    // mistaken for a failure.
    let dir = install_dir("fresh");
    let target = dir.join("medulla");
    let new = dir.join("staged");
    std::fs::write(&new, b"NEW").unwrap();

    install_binary(&new, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert!(
        !backup_path(&target).exists(),
        "nothing was there to back up"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_writable_directory_is_reported_as_installable() {
    let dir = install_dir("writable");
    assert!(exe_is_writable(&dir.join("medulla")));
    // A path whose parent does not exist cannot be written to.
    assert!(!exe_is_writable(&dir.join("no-such-dir/medulla")));
    let _ = std::fs::remove_dir_all(&dir);
}
