//! The Routing Workspaces page: what it lists, what it lets the operator change,
//! and what it refuses to change from here.
//!
//! The page exists so the orchestrator has something to reason about beyond
//! "there is a claude daemon on this laptop". What is declared here for this
//! device is exactly what reaches it as `capabilities.accessibleDirs`.
//!
//! **Currently ignored.** Workspaces is commented out of `ROUTING_SUBPAGES`, so
//! `focus_routing_subpage` cannot land on it. The page's rendering, keys and
//! `[host].workspaces` persistence are untouched — putting its name back in the
//! list restores both the page and this suite.
//!
//! Adding a *host* per directory (Add Host › Local) largely supersedes what this
//! page did: an extra workspace was advisory routing context, whereas a host
//! actually runs work there.

use crate::helpers::*;

use medulla::config::LoadedConfig;
use medulla::runtime::Runtime;
use std::sync::Arc;

/// An app whose config declares `dirs` as this device's extra workspaces, with
/// persistence pointed at a throwaway config file.
fn app_with_workspaces(dirs: &[&str], config: &std::path::Path) -> App {
    let rt: Arc<dyn Runtime> = Arc::new(FleetRuntime::new(Vec::new(), None));
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.host.workspace = "/srv/primary".into();
    loaded.config.host.workspaces = dirs.iter().map(|d| d.to_string()).collect();
    let mut app = App::new(rt, loaded);
    app.set_config_path(config.to_path_buf());
    app
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn the_page_lists_this_devices_directories_and_marks_where_work_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_workspaces(&["/srv/side-project"], &dir.path().join("config.toml"));
    app.focus_routing_subpage("Workspaces");
    let out = render(&mut app, 160, 40);

    assert!(out.contains("/srv/primary"), "{out}");
    assert!(out.contains("/srv/side-project"), "{out}");
    // The one distinction that changes what a delegated task does.
    assert!(out.contains("where tasks run"), "{out}");
    assert!(out.contains("this device"), "{out}");
    assert!(out.contains("a add a directory"), "{out}");
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn a_long_workspace_path_keeps_both_ends_inside_its_two_line_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = "/Users/example/work/tinyhumansai/very/deep/project/medulla-public";
    let mut app = app_with_workspaces(&[path], &dir.path().join("config.toml"));
    app.focus_routing_subpage("Workspaces");
    let out = render(&mut app, 76, 32);

    assert!(out.contains("/Users/"), "path root should survive: {out}");
    assert!(
        out.contains("medulla-public"),
        "project tail should survive: {out}"
    );
    assert!(
        out.contains('…'),
        "the omitted middle should be explicit: {out}"
    );
    assert!(!out.contains(path), "the row should not overflow: {out}");
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn a_directory_that_is_not_on_disk_is_flagged_rather_than_hidden() {
    // Declaring a path on a mount that is not up yet is legitimate; silently
    // dropping it would leave the operator wondering why it never appeared.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_workspaces(&["/definitely/not/here"], &dir.path().join("config.toml"));
    app.focus_routing_subpage("Workspaces");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("missing on disk"), "{out}");
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn adding_a_directory_writes_it_to_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let added = dir.path().join("project");
    std::fs::create_dir_all(&added).unwrap();

    let mut app = app_with_workspaces(&[], &config);
    app.focus_routing_subpage("Workspaces");
    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    let (title, _) = app.prompt_state().expect("the add prompt is up");
    assert!(title.contains("Add workspace"), "{title}");

    for ch in added.to_string_lossy().chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
    assert!(app.on_event(key(KeyCode::Enter)).is_none());

    let text = std::fs::read_to_string(&config).expect("config written");
    assert!(
        text.contains(&added.to_string_lossy().to_string())
            || text.contains(
                &std::fs::canonicalize(&added)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            ),
        "the directory is persisted: {text}"
    );
    assert!(app.status().contains("Added workspace"), "{}", app.status());
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn the_same_directory_is_not_declared_twice() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut app = app_with_workspaces(&["/srv/side-project"], &config);
    app.focus_routing_subpage("Workspaces");
    let before = app.workspace_row_count();
    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    for ch in "/srv/side-project".chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
    assert!(app.on_event(key(KeyCode::Enter)).is_none());
    assert!(
        app.status().contains("Already declared"),
        "{}",
        app.status()
    );
    assert_eq!(
        app.workspace_row_count(),
        before,
        "one directory, one row — declaring it again changes nothing"
    );
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn removing_a_declared_directory_takes_it_out_of_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut app = app_with_workspaces(&["/srv/side-project"], &config);
    app.focus_routing_subpage("Workspaces");
    // Row 0 is the primary; step onto the extra before removing.
    let _ = app.on_event(key(KeyCode::Down));
    assert!(app.on_event(key(KeyCode::Char('d'))).is_none());

    let text = std::fs::read_to_string(&config).expect("config written");
    assert!(
        !text.contains("/srv/side-project"),
        "removal must be durable, not merged away: {text}"
    );
    assert!(
        app.status().contains("Removed workspace"),
        "{}",
        app.status()
    );
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn the_working_directory_cannot_be_removed_from_this_page() {
    // Removing it would leave the host with nowhere to run tasks; it is the
    // `[host].workspace` setting, not one of the advertised extras.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_workspaces(&["/srv/side-project"], &dir.path().join("config.toml"));
    app.focus_routing_subpage("Workspaces");
    assert!(app.on_event(key(KeyCode::Char('d'))).is_none());
    assert!(
        app.status().contains("where tasks run"),
        "explained, not silently ignored: {}",
        app.status()
    );
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn another_machines_workspace_is_shown_but_not_removable_here() {
    // The scripted fleet declares a workspace on `workshop`. It belongs to that
    // machine's config, so this page shows it and refuses to change it.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_workers(None);
    app.set_config_path(dir.path().join("config.toml"));
    app.focus_routing_subpage("Workspaces");
    let out = render(&mut app, 160, 40);
    assert!(out.contains("/srv/repos/medulla"), "{out}");
    assert!(out.contains("declared remotely"), "{out}");
    assert!(out.contains("on workshop"), "{out}");

    // Walk onto it and try: the refusal names where to go instead.
    for _ in 0..app.workspace_row_count() {
        if app.selected_workspace_is_remote().unwrap_or(false) {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    assert!(app.on_event(key(KeyCode::Char('d'))).is_none());
    assert!(
        app.status().contains("remove it there"),
        "explained, not silently ignored: {}",
        app.status()
    );
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn a_relative_path_is_stored_absolute() {
    // `canonicalize` fails for a directory that is not there yet, and persisting
    // `projects/repo` verbatim would silently name a different place on the next
    // launch — whatever directory medulla happened to start in.
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut app = app_with_workspaces(&[], &config);
    app.focus_routing_subpage("Workspaces");
    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    for ch in "projects/not-created-yet".chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
    assert!(app.on_event(key(KeyCode::Enter)).is_none());

    let text = std::fs::read_to_string(&config).expect("config written");
    assert!(
        !text.contains("\"projects/not-created-yet\""),
        "the relative form must not be what is stored: {text}"
    );
    let cwd = std::env::current_dir().unwrap();
    assert!(
        text.contains(
            &cwd.join("projects/not-created-yet")
                .to_string_lossy()
                .to_string()
        ),
        "stored absolute against the current directory: {text}"
    );
}

#[ignore = "Routing › Workspaces is commented out of ROUTING_SUBPAGES"]
#[test]
fn entering_the_page_pulls_a_fresh_read_of_the_fleet() {
    // A backend session starts with an empty capacity snapshot, so without this
    // the page showed only this device's directories until the operator happened
    // to visit another capacity page first.
    let mut app = app_with_workers(None);
    tab(&mut app, "Hosts");
    // Walk the nav down to Workspaces, then step in.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Down));
    assert_eq!(app.routing_subpage(), "Workspaces");
    assert!(matches!(
        app.on_event(key(KeyCode::Enter)),
        Some(Cmd::RefreshFleet)
    ));
}
