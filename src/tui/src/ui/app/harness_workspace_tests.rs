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
fn a_long_workspace_list_windows_onto_the_selected_folder() {
    // Regression: the workspace step extended its lines with *every* choice,
    // where the harness step windows. On a short popup the rows past the fold
    // were never painted and never recorded a hit box, so ↓ walked the selection
    // onto an invisible, unclickable directory and Enter started a session there.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let root = tempfile::tempdir().unwrap();
    let mut app = picker_on_workspace_step(root.path());
    {
        let picker = app.session_picker.as_mut().unwrap();
        picker.workspace_choices = (0..30)
            .map(|index| super::types::WorkspaceChoice {
                path: format!("/w/folder-{index:02}"),
                source: "recent",
            })
            .collect();
        picker.workspace_index = 29;
    }

    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(
        screen.contains("folder-29"),
        "the selected folder is on screen"
    );
    assert!(
        !screen.contains("folder-00"),
        "and the list has scrolled off the first page"
    );

    // Every drawn row is clickable, and the last one still names the selection.
    let (_, hits) = app.hit_session_picker.clone().expect("picker hit map");
    assert!(
        hits.iter().any(|(_, index)| *index == 29),
        "the selected row has a hit box: {hits:?}"
    );
    assert!(
        hits.iter().all(|(_, index)| *index >= 18),
        "and no hit box points at a row that was not drawn: {hits:?}"
    );
}
