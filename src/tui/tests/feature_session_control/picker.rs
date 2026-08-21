//! Starting a session by hand: the two-step picker, its workspace completion
//! and recents, and the pointer targets on both steps.
//!
//! The load-bearing fact throughout is that a session started this way is the
//! operator's — there is no control question, and `claim_idle` must not hand it
//! out.

use crate::helpers::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use medulla_tui::worker::pty::{PtyManager, SessionControl};

#[test]
fn ctrl_t_opens_the_picker_and_two_enters_start_an_unmanaged_harness() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let out = render(&mut app, 140, 44);
    assert!(out.contains("Choose a session type"), "{out}");
    assert!(
        out.contains("the orchestrator will not dispatch into it"),
        "the picker must say what unmanaged means: {out}"
    );

    let _ = app.on_event(key(KeyCode::Enter));
    let out = render(&mut app, 140, 44);
    assert!(out.contains("Choose workspace"), "{out}");
    assert!(out.contains("/"), "{out}");

    // The workspace step is the last one — its own title promises "Enter start",
    // and there is no control question behind it to make a liar of it.
    assert!(out.contains("Enter start"), "{out}");
    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);

    let row = sessions.rows().remove(0);
    assert!(row.origin.is_user());
    assert_eq!(row.control, SessionControl::User);
    assert!(!row.busy, "nothing is running in it yet");
    assert!(
        app.status().contains("unmanaged"),
        "the operator is told what they just started: {}",
        app.status()
    );

    sessions.shutdown();
}

#[test]
fn the_picker_rows_are_click_targets_on_both_steps() {
    // The picker is opened from `Ctrl-T` or from clicking `+ New session`, so
    // the operator arrives with a hand on the mouse — and it used to swallow the
    // pointer wholesale, which reads as a frozen screen rather than as a
    // keyboard-only step. A click is Enter on the row it lands on.
    let root = tempfile::tempdir().unwrap();
    let alpha = root.path().join("project-alpha");
    std::fs::create_dir(&alpha).unwrap();
    let sessions = PtyManager::new();
    let mut app = app_with_workspace(sessions.clone(), root.path().to_str().unwrap());
    app.loaded.config.harness.recent_workspaces = vec![alpha.to_string_lossy().into_owned()];

    let _ = app.on_event(ctrl('t'));
    let lines = render_lines(&mut app, 140, 44);
    let (column, row) = label_at(&lines, "Codex");

    let _ = app.on_event(click(column, row));

    let lines = render_lines(&mut app, 140, 44);
    assert!(
        lines.join("\n").contains("Choose workspace"),
        "clicking a provider advances exactly as Enter does: {}",
        lines.join("\n")
    );
    assert!(
        sessions.rows().is_empty(),
        "the harness step is a navigation, so nothing has started yet"
    );

    // The second step's click has to carry the row's own index, not the
    // highlighted one. `project-alpha` is the recent workspace and starts
    // selected, so clicking the `default` row below it proves the pointer picked
    // what it landed on rather than confirming wherever the cursor happened to
    // be.
    let (column, row) = label_at(&lines, "default");
    let _ = app.on_event(click(column, row));

    wait_for("the harness to open in the clicked folder", || {
        sessions.rows().len() == 1
    });
    assert_eq!(
        sessions.rows().remove(0).cwd,
        root.path().to_string_lossy().into_owned(),
        "the click must start in the row it landed on, not the highlighted one"
    );

    sessions.shutdown();
}

#[test]
fn the_wheel_walks_the_picker_and_a_click_off_a_row_starts_nothing() {
    // The wheel is the other half of making the modal usable with a pointer: the
    // harness list windows when it is long, so without it there is no way to
    // reach the rows below the fold without reaching for the keyboard.
    let root = tempfile::tempdir().unwrap();
    let alpha = root.path().join("project-alpha");
    std::fs::create_dir(&alpha).unwrap();
    let sessions = PtyManager::new();
    let mut app = app_with_workspace(sessions.clone(), root.path().to_str().unwrap());
    app.loaded.config.harness.recent_workspaces = vec![alpha.to_string_lossy().into_owned()];

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Enter));
    let lines = render_lines(&mut app, 140, 44);
    let (_, first) = label_at(&lines, "project-alpha");
    let (column, _) = label_at(&lines, "Choose workspace");

    // A notch down moves the highlight, exactly as ↓ does.
    let _ = app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: column + 4,
        row: first,
        modifiers: KeyModifiers::NONE,
    }));
    let lines = render_lines(&mut app, 140, 44);
    let marked = lines
        .iter()
        .find(|line| line.contains("❯ /"))
        .expect("a highlighted workspace row");
    assert!(
        marked.contains("default"),
        "the wheel must move off the recent workspace onto the row below: {marked}"
    );

    // A click inside the box but not on a row is swallowed and decides nothing —
    // the title bar is not an answer, and it must not reach the rail behind.
    let (column, row) = label_at(&lines, "Choose workspace");
    let _ = app.on_event(click(column, row));
    assert!(
        app.session_picker_open_for_test(),
        "a click off a row leaves the picker up: {}",
        app.status()
    );
    assert!(
        sessions.rows().is_empty(),
        "and starts nothing: {}",
        app.status()
    );

    sessions.shutdown();
}

#[test]
fn an_unmanaged_harness_gets_its_own_rail_row() {
    // Lanes come from task events, so a session nothing dispatched into folds to
    // no lane at all. Without a row of its own it would be running and invisible.
    //
    // It no longer gets a *group* of its own: §A0 collapsed the `── your
    // harnesses ──` divider, because an operator-started session and a
    // dispatched task are one thing seen from two sides. It is a session row on
    // the tree like any other.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let _ = user_session(&sessions);

    let out = render(&mut app, 140, 44);
    assert!(!out.contains("your harnesses"), "no second group: {out}");
    assert!(!out.contains("your sessions"), "no second group: {out}");
    assert!(out.contains("unmanaged"), "{out}");

    sessions.shutdown();
}

#[test]
fn the_picker_cancels_without_starting_anything() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Esc));

    let out = render(&mut app, 140, 44);
    assert!(!out.contains("Choose a session type"), "{out}");
    assert!(sessions.rows().is_empty(), "Esc must not start a harness");
}

#[test]
fn the_picker_directory_can_be_edited_before_starting() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Char('e')));
    type_str(&mut app, "tmp");

    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("/tmp"),
        "the picker must show the edited path: {out}"
    );
    assert!(
        sessions.rows().is_empty(),
        "editing must not start the harness"
    );

    // Enter takes the directory and starts the harness in it.
    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);
    assert_eq!(sessions.rows().remove(0).cwd, "/tmp");

    sessions.shutdown();
}

#[test]
fn workspace_picker_does_not_insert_modifier_chords_into_the_query() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Enter));
    let _ = app.on_event(ctrl('x'));

    let out = render(&mut app, 140, 44);
    assert!(out.contains("search › ▌"), "{out}");
    assert!(!out.contains("search › x▌"), "{out}");
    assert!(sessions.rows().is_empty());

    sessions.shutdown();
}

#[test]
fn workspace_picker_autocompletes_folders_and_remembers_successful_choices() {
    let root = tempfile::tempdir().unwrap();
    let alpha = root.path().join("project-alpha");
    let beta = root.path().join("project-beta");
    std::fs::create_dir(&alpha).unwrap();
    std::fs::create_dir(&beta).unwrap();
    let sessions = PtyManager::new();
    let mut app = app_with_workspace(sessions.clone(), root.path().to_str().unwrap());
    app.loaded.config.harness.recent_workspaces = vec![alpha.to_string_lossy().into_owned()];
    let config = root.path().join("config.toml");
    app.set_config_path(config.clone());

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Enter));
    let out = render(&mut app, 140, 44);
    assert!(out.contains("project-alpha"), "{out}");
    assert!(out.contains("recent"), "{out}");

    type_str(&mut app, "pb");
    let out = render(&mut app, 140, 44);
    assert!(out.contains("project-beta"), "{out}");
    assert!(out.contains("folder"), "{out}");

    let _ = app.on_event(key(KeyCode::Tab));
    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("search ›") && out.contains("project-beta"),
        "{out}"
    );

    // Enter takes the completed folder and starts there.
    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open in the completed folder", || {
        sessions.rows().len() == 1
    });
    assert_eq!(
        sessions.rows().remove(0).cwd,
        beta.to_string_lossy().into_owned()
    );
    let saved = std::fs::read_to_string(config).unwrap();
    assert!(saved.contains("recentWorkspaces"));
    assert!(saved.contains("project-beta"));
    assert!(saved.find("project-beta") < saved.find("project-alpha"));

    sessions.shutdown();
}

#[test]
fn workspace_picker_keeps_recent_workspaces_newest_first() {
    let root = tempfile::tempdir().unwrap();
    let newest = root.path().join("zeta-workspace");
    let older = root.path().join("alpha-workspace");
    std::fs::create_dir(&newest).unwrap();
    std::fs::create_dir(&older).unwrap();
    let sessions = PtyManager::new();
    let mut app = app_with_workspace(sessions.clone(), root.path().to_str().unwrap());
    app.loaded.config.harness.recent_workspaces = vec![
        newest.to_string_lossy().into_owned(),
        older.to_string_lossy().into_owned(),
    ];

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Enter));
    let out = render(&mut app, 140, 44);

    let newest = out
        .find("zeta-workspace")
        .expect("newest workspace missing");
    let older = out
        .find("alpha-workspace")
        .expect("older workspace missing");
    assert!(
        newest < older,
        "newest recent workspace must remain first: {out}"
    );
    sessions.shutdown();
}
