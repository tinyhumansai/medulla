//! Focused tests for bounded folder completion and fuzzy ranking.

use super::harness_workspace::{
    absolute, folder_completions, fuzzy_subsequence_score, match_score,
};

#[test]
fn fuzzy_matching_accepts_tight_subsequences_and_rejects_missing_characters() {
    assert!(fuzzy_subsequence_score("workspace-manager", "wsm").is_some());
    assert!(
        fuzzy_subsequence_score("workspace-manager", "workspace")
            < fuzzy_subsequence_score("workspace-manager", "wsm")
    );
    assert_eq!(fuzzy_subsequence_score("workspace-manager", "xyz"), None);
}

#[test]
fn folder_completion_lists_matching_children_but_never_files() {
    let root = tempfile::tempdir().unwrap();
    let alpha = root.path().join("project-alpha");
    let beta = root.path().join("project-beta");
    std::fs::create_dir(&alpha).unwrap();
    std::fs::create_dir(&beta).unwrap();
    std::fs::write(root.path().join("project-not-a-folder"), "no").unwrap();

    let query = root.path().join("pb").to_string_lossy().into_owned();
    let results = folder_completions(&query);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, beta.to_string_lossy());
}

#[test]
fn known_workspace_basename_prefixes_beat_filesystem_duplicates() {
    assert_eq!(match_score("/work/project-beta", "project-b"), Some(1));
}

#[test]
fn loose_known_matches_do_not_beat_concrete_folder_matches() {
    let random_parent = match_score("/tmp/.tmpbQM6Hg", "pb").unwrap();
    let project_beta = fuzzy_subsequence_score("project-beta", "pb").unwrap() + 5;

    assert!(project_beta < random_parent);
}

#[test]
fn registered_relative_workspace_resolves_from_process_directory() {
    let root = tempfile::tempdir().unwrap();
    let process_dir = root.path().join("work");
    let already_absolute = root.path().join("srv").join("repo");

    assert_eq!(
        std::path::PathBuf::from(absolute("other", &process_dir)),
        process_dir.join("other")
    );
    assert_eq!(
        std::path::PathBuf::from(absolute(already_absolute.to_str().unwrap(), &process_dir)),
        already_absolute
    );
}

#[test]
fn folder_completion_keeps_only_the_best_bounded_set() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..20 {
        std::fs::create_dir(root.path().join(format!("project-{index:02}"))).unwrap();
    }

    let query = root.path().join("project").to_string_lossy().into_owned();
    let results = folder_completions(&query);

    assert_eq!(results.len(), 10);
    assert!(results.windows(2).all(|pair| pair[0] <= pair[1]));
}
