//! Paste into the "start a session" picker, which is a modal on its first step
//! and a text field on its second — so it is the one overlay where "does this
//! own a text field?" has two answers.

use crate::helpers::*;

use std::collections::HashMap;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::protocol::HarnessProvider;
use medulla_tui::ui::harness_pane::LocalSessions;
use medulla_tui::worker::pty::PtyManager;

/// An Agents-tab app that can offer a harness type to start.
///
/// No child is ever launched here: the picker only needs a provider list to
/// build its choices from, and every test stops before the Enter that would
/// spawn one.
fn picker_app() -> App {
    let mut app = demo_agents_app();
    app.set_local_sessions(LocalSessions {
        sessions: PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app
}

fn rendered(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 44)).expect("test terminal");
    terminal.draw(|f| app.draw(f)).expect("draws");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn open_picker(app: &mut App) {
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert!(
        rendered(app).contains("Choose a harness type"),
        "the picker is open on its first step"
    );
}

#[test]
fn the_harness_type_step_swallows_a_paste_instead_of_typing_it_behind_the_modal() {
    // The first step is a list of providers with no field on it. It owns the
    // keyboard while it is up, so it owns the paste too — anything else
    // edits the composer hidden behind the modal, and the operator finds out
    // when they submit it later.
    let mut app = picker_app();
    open_picker(&mut app);

    assert!(paste(&mut app, "stray text").is_none());

    assert_eq!(
        app.draft_text(),
        "",
        "nothing reached the composer behind it"
    );
    let screen = rendered(&mut app);
    assert!(
        !screen.contains("stray text"),
        "and nothing was typed into the modal either: {screen}"
    );
}

#[test]
fn the_workspace_step_takes_a_pasted_directory_into_its_query() {
    // The second step *is* a text field — it says "type to filter" and
    // accepts typed path characters — so pasting a directory into it is the
    // common case, not an edge one.
    let mut app = picker_app();
    open_picker(&mut app);
    key_press(&mut app, KeyCode::Enter);
    assert!(
        rendered(&mut app).contains("Choose workspace"),
        "the picker has advanced to the workspace step"
    );

    // Copied from a file manager, so it arrives with a trailing newline. A
    // one-line field has no row to put that on; it flattens to a space,
    // which the resolve on submit trims off.
    assert!(paste(&mut app, "/etc\n").is_none());

    let screen = rendered(&mut app);
    assert!(
        screen.contains("search › /etc"),
        "the pasted path is in the query: {screen}"
    );
}
