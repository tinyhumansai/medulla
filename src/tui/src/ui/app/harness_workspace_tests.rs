//! Focused tests for bounded folder completion and fuzzy ranking.

use super::harness_workspace::{
    absolute, folder_completions, fuzzy_subsequence_score, match_score, workspace_match_score,
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
fn favorite_names_are_searchable_as_well_as_their_paths() {
    assert_eq!(
        workspace_match_score("/work/medulla-public", Some("Primary Medulla"), "primary"),
        Some(1)
    );
    assert!(
        workspace_match_score("/work/medulla-public", Some("Primary Medulla"), "medulla").is_some()
    );
}

#[test]
fn an_exact_path_match_outranks_a_loose_label_match_for_a_favorite() {
    // A query that names the directory exactly must rank the favorite at the
    // path score rather than the loose label score, or a plain filesystem
    // completion for the same directory would outrank it and win the row after
    // path de-duplication.
    assert_eq!(
        workspace_match_score("/work/medulla", Some("primary medulla"), "medulla"),
        Some(0)
    );
    // The label's own exact match is still honoured when the path does not
    // match at all.
    assert_eq!(
        workspace_match_score("/work/medulla-public", Some("Primary Medulla"), "primary"),
        Some(1)
    );
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

/// An Sessions-tab app with a picker parked on its workspace step, whose default
/// workspace is `workspace`.
fn picker_on_workspace_step(workspace: &std::path::Path) -> super::types::App {
    use super::types::{App, SessionPicker, SessionPickerStep};

    let mut loaded = medulla::config::LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    let mut app = App::new(
        std::sync::Arc::new(medulla::runtime::mock::MockRuntime::empty()),
        loaded,
    );
    app.set_local_sessions(crate::ui::harness_pane::LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions: crate::worker::pty::PtyManager::new(),
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env: std::collections::HashMap::new(),
        workspace: workspace.to_string_lossy().into_owned(),
        providers: vec![medulla::protocol::HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app.session_picker = Some(SessionPicker {
        choices: Vec::new(),
        index: 0,
        step: SessionPickerStep::Workspace,
        cwd: workspace.to_string_lossy().into_owned(),
        workspace_query: String::new(),
        workspace_choices: Vec::new(),
        workspace_index: 0,
        workspace_picked: false,
    });
    app
}

#[test]
fn a_pasted_directory_starts_there_rather_than_in_its_first_child() {
    // A path copied from a file manager arrives with a trailing separator, and
    // the completions under it are that directory's *children*. Enter used to
    // take the highlighted completion unconditionally, so pasting `/repo/`
    // started the harness in `/repo/first-child` — silently the wrong project.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("alpha")).unwrap();
    std::fs::create_dir(root.path().join("beta")).unwrap();
    let mut app = picker_on_workspace_step(root.path());

    let pasted = format!("{}{}\n", root.path().display(), std::path::MAIN_SEPARATOR);
    app.on_event(crossterm::event::Event::Paste(pasted));

    assert!(
        !app.session_picker
            .as_ref()
            .unwrap()
            .workspace_choices
            .is_empty(),
        "the children are still offered as completions"
    );
    assert_eq!(
        app.selected_picker_workspace()
            .map(std::path::PathBuf::from),
        Some(root.path().to_path_buf()),
        "but Enter starts in the directory that was actually pasted"
    );
}

#[test]
fn arrowing_onto_a_completion_still_wins_over_the_typed_query() {
    // The other half: preferring the query must not strand an operator who
    // pasted a parent and then deliberately picked a child from the list.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("alpha")).unwrap();
    let mut app = picker_on_workspace_step(root.path());
    let pasted = format!("{}{}\n", root.path().display(), std::path::MAIN_SEPARATOR);
    app.on_event(crossterm::event::Event::Paste(pasted));

    app.on_event(crossterm::event::Event::Key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ),
    ));

    assert_eq!(
        app.selected_picker_workspace()
            .map(std::path::PathBuf::from),
        Some(root.path().join("alpha")),
        "a deliberately chosen completion is still what Enter uses"
    );
}

#[test]
fn saving_a_named_favorite_persists_it_and_makes_its_name_searchable() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("medulla");
    std::fs::create_dir(&workspace).unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[harness]\n").unwrap();
    let mut app = picker_on_workspace_step(&workspace);
    app.set_config_path(config.clone());

    app.save_favorite_workspace("Daily Medulla", workspace.to_str().unwrap());

    assert_eq!(app.loaded.config.harness.favorite_workspaces.len(), 1);
    assert_eq!(
        app.loaded.config.harness.favorite_workspaces[0].name,
        "Daily Medulla"
    );
    assert_eq!(
        app.loaded.config.harness.favorite_workspaces[0].path,
        workspace.to_string_lossy()
    );
    assert!(std::fs::read_to_string(config)
        .unwrap()
        .contains("favoriteWorkspaces"));

    let picker = app.session_picker.as_mut().unwrap();
    picker.workspace_query = "daily".into();
    picker.workspace_picked = false;
    app.refresh_harness_workspace_choices();
    let choice = app
        .session_picker
        .as_ref()
        .unwrap()
        .workspace_choices
        .first()
        .unwrap();
    assert_eq!(choice.label.as_deref(), Some("Daily Medulla"));
    assert_eq!(choice.path, workspace.to_string_lossy());
}

#[test]
fn saving_a_favorite_reanchors_the_cursor_on_the_saved_workspace() {
    // Saving promotes the new favorite to the head of the list, so the arrowed
    // cursor must follow it (index 0) rather than keep pointing at the row it
    // covered before — otherwise Enter starts the harness in whatever directory
    // landed in that row instead of the workspace just favorited.
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("medulla");
    std::fs::create_dir(&workspace).unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[harness]\n").unwrap();
    let mut app = picker_on_workspace_step(&workspace);
    app.set_config_path(config.clone());

    // The operator arrows onto a non-first completion before saving.
    let picker = app.session_picker.as_mut().unwrap();
    picker.workspace_index = 3;
    picker.workspace_picked = true;

    app.save_favorite_workspace("Daily Medulla", workspace.to_str().unwrap());

    let picker = app.session_picker.as_ref().unwrap();
    assert_eq!(
        picker.workspace_index, 0,
        "the cursor follows the promoted favorite to the head row"
    );
    assert!(
        picker.workspace_picked,
        "Enter must still honour the highlighted row after the refresh"
    );
    assert_eq!(
        app.selected_picker_workspace()
            .map(std::path::PathBuf::from),
        Some(workspace.clone()),
        "Enter starts in the workspace that was just favorited"
    );
}

#[test]
fn a_failed_favorite_save_does_not_replace_the_in_memory_favorites() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("medulla");
    std::fs::create_dir(&workspace).unwrap();
    let mut app = picker_on_workspace_step(&workspace);
    // An unparsable config file makes persistence fail before any write — a
    // favorite the disk never recorded must not be visible in memory either,
    // or a later successful save could silently persist it.
    let config = root.path().join("config.toml");
    std::fs::write(&config, "not [valid toml {{{").unwrap();
    app.set_config_path(config);

    app.save_favorite_workspace("Daily Medulla", workspace.to_str().unwrap());

    assert!(
        app.loaded.config.harness.favorite_workspaces.is_empty(),
        "a favorite that could not be persisted must not appear to be saved"
    );
    assert!(app.status().contains("Could not save favorite"));
}

#[test]
fn saving_under_a_resolved_absolute_path_replaces_a_relative_favorite() {
    // A favorite persisted from relative input (`repo` against the host
    // workspace) resolves to the same directory as an absolute re-save of it,
    // so the de-duplication must compare effective resolved paths — otherwise
    // the old entry survives under its stale spelling and the same directory
    // occupies two ranked rows.
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[harness]\n").unwrap();
    let mut app = picker_on_workspace_step(root.path());
    app.set_config_path(config.clone());
    app.loaded.config.harness.favorite_workspaces = vec![medulla::config::FavoriteWorkspace {
        name: "old alias".into(),
        path: "repo".into(),
    }];

    app.save_favorite_workspace("new alias", repo.to_str().unwrap());

    assert_eq!(
        app.loaded.config.harness.favorite_workspaces.len(),
        1,
        "a relative entry must not survive beside the same directory re-saved absolutely"
    );
    assert_eq!(
        app.loaded.config.harness.favorite_workspaces[0].name,
        "new alias"
    );
    assert_eq!(
        app.loaded.config.harness.favorite_workspaces[0].path,
        repo.to_string_lossy()
    );
}

#[test]
fn renaming_a_filtered_favorite_cannot_fall_through_to_an_unrelated_row() {
    // Shift+F on a favorite found only through a query matching its old label,
    // renamed so the new label no longer matches the unchanged query: the
    // refresh drops the old row and the promoted favorite is absent, leaving a
    // list that is empty or leads with an unrelated match. Forcing the cursor
    // onto row 0 would make Enter reject the workspace or launch the unrelated
    // row, so the picker must keep the saved workspace selected.
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("medulla");
    std::fs::create_dir(&workspace).unwrap();
    let other = root.path().join("other-medulla");
    std::fs::create_dir(&other).unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[harness]\n").unwrap();
    let mut app = picker_on_workspace_step(&workspace);
    app.set_config_path(config.clone());
    app.loaded.config.harness.favorite_workspaces = vec![
        medulla::config::FavoriteWorkspace {
            name: "daily".into(),
            path: workspace.to_string_lossy().into_owned(),
        },
        medulla::config::FavoriteWorkspace {
            name: "dailies".into(),
            path: other.to_string_lossy().into_owned(),
        },
    ];
    let picker = app.session_picker.as_mut().unwrap();
    picker.workspace_query = "daily".into();
    app.refresh_harness_workspace_choices();
    assert_eq!(
        app.session_picker
            .as_ref()
            .unwrap()
            .workspace_choices
            .first()
            .unwrap()
            .label
            .as_deref(),
        Some("daily"),
        "the filtered favorite leads the list before the rename"
    );

    app.save_favorite_workspace("zap", workspace.to_str().unwrap());

    let picker = app.session_picker.as_ref().unwrap();
    assert_eq!(
        picker.workspace_query,
        workspace.to_string_lossy(),
        "a rename that outlives its filter re-points the query at the saved workspace"
    );
    assert_eq!(
        app.selected_picker_workspace()
            .map(std::path::PathBuf::from),
        Some(workspace.clone()),
        "Enter must not launch the unrelated row that now leads the old filter"
    );
}
