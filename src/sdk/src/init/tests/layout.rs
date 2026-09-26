//! Unit tests for the workspace layout scan. Every test builds a real scratch
//! tree — the scan's whole job is reading a filesystem, so faking that away
//! would test nothing.

use std::fs;
use std::path::PathBuf;

use crate::init::layout::{layout_block, scan_layout};

/// A unique scratch directory per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("medulla-layout-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn scan_lists_files_and_one_level_of_directories() {
    let dir = scratch("basic");
    fs::write(dir.join("README.md"), "x").unwrap();
    fs::create_dir_all(dir.join("src/client")).unwrap();
    fs::write(dir.join("src/lib.rs"), "x").unwrap();

    let lines = scan_layout(&dir);
    assert!(lines.contains(&"README.md".to_string()));
    assert!(lines.contains(&"src/".to_string()));
    assert!(lines.contains(&"src/lib.rs".to_string()));
    // One level in: the child directory is named, its contents are not.
    assert!(lines.contains(&"src/client/".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_sorts_directories_before_files() {
    let dir = scratch("order");
    fs::write(dir.join("zzz.md"), "x").unwrap();
    fs::create_dir_all(dir.join("aaa")).unwrap();

    let lines = scan_layout(&dir);
    assert_eq!(lines, vec!["aaa/".to_string(), "zzz.md".to_string()]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_skips_build_output_and_hidden_entries() {
    let dir = scratch("ignored");
    fs::create_dir_all(dir.join("target/debug")).unwrap();
    fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".env"), "SECRET=1").unwrap();
    fs::write(dir.join("main.rs"), "x").unwrap();

    let lines = scan_layout(&dir);
    assert_eq!(lines, vec!["main.rs".to_string()]);
    // A secret-bearing dotfile must never reach an orchestrator's context.
    assert!(!lines.iter().any(|line| line.contains(".env")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_summarises_a_directory_with_many_children() {
    let dir = scratch("many");
    fs::create_dir_all(dir.join("mod")).unwrap();
    for i in 0..20 {
        fs::write(dir.join("mod").join(format!("f{i:02}.rs")), "x").unwrap();
    }

    let lines = scan_layout(&dir);
    let named = lines.iter().filter(|l| l.starts_with("mod/f")).count();
    assert_eq!(named, 8, "only the first few children are named");
    assert!(lines.contains(&"mod/… 12 more".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_is_bounded_for_a_huge_tree() {
    let dir = scratch("huge");
    for i in 0..200 {
        fs::write(dir.join(format!("f{i:03}.txt")), "x").unwrap();
    }
    assert!(scan_layout(&dir).len() <= 40);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scan_of_an_unreadable_or_missing_directory_is_empty() {
    assert!(scan_layout(std::path::Path::new("/no/such/directory")).is_empty());
}

#[test]
fn layout_block_indents_every_line_and_placeholders_when_empty() {
    let block = layout_block(&["src/".to_string(), "src/lib.rs".to_string()]);
    assert_eq!(block, "  src/\n  src/lib.rs");
    // An empty scan must still leave a well-formed block scalar.
    assert_eq!(layout_block(&[]), "  (empty workspace)");
}
