//! Paste into the Workflows copilot — the app's second multiline composer, and
//! the one a long instruction is most likely to be pasted into.

#![cfg(feature = "workflows")]

use crate::helpers::*;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// A Workflows-tab app reading a store scoped to `home`.
///
/// Scoped deliberately: the store a real session resolves also reads the
/// current directory's `.medulla/workflows`, which would make the test
/// depend on the developer's own catalogue.
fn workflows_app(home: &std::path::Path) -> App {
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded());
    app.set_medulla_home(home.to_path_buf());
    app.set_workflow_store(Arc::new(medulla::workflows::FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    )));
    app.tab_index = tab_index("Workflows");
    app
}

fn rendered(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(150, 34)).expect("test terminal");
    terminal.draw(|f| app.draw(f)).expect("draws");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn a_paste_lands_in_the_copilot_composer_with_its_line_breaks() {
    let home = tempfile::tempdir().expect("temp home");
    let mut app = workflows_app(home.path());
    // `c` is the binding that reaches the copilot from the catalogue.
    key_press(&mut app, KeyCode::Char('c'));

    assert!(paste(&mut app, "run the sweep\r\nthen notify me").is_none());

    let screen = rendered(&mut app);
    assert!(
        screen.contains("run the sweep") && screen.contains("then notify me"),
        "the whole instruction is in the composer: {screen}"
    );
}

#[test]
fn a_paste_on_the_catalogue_is_ignored_rather_than_stored_out_of_sight() {
    // The sidebar is a list, not a field. Its only draft is the copilot's,
    // one pane over and not on screen for this focus.
    let home = tempfile::tempdir().expect("temp home");
    let mut app = workflows_app(home.path());

    assert!(paste(&mut app, "stray text").is_none());

    key_press(&mut app, KeyCode::Char('c'));
    let screen = rendered(&mut app);
    assert!(
        !screen.contains("stray text"),
        "nothing was banked into the copilot behind the list: {screen}"
    );
}

#[test]
fn a_popup_left_open_on_another_tab_neither_covers_nor_swallows() {
    // The agent-template popup is a Hosts › Agent Templates detail view, but
    // `Tab` switches tabs while it is up and nothing cleared the flag, so it
    // kept drawing over whatever tab you landed on — including this one,
    // where none of its keys are bound. It is now scoped to its own page, so
    // the copilot here is genuinely visible and the paste belongs to it.
    let home = tempfile::tempdir().expect("temp home");
    let mut app = workflows_app(home.path());
    app.focus_routing_subpage("Agent Templates");
    let _ = app.on_event(key(KeyCode::Enter));
    app.tab_index = tab_index("Workflows");
    key_press(&mut app, KeyCode::Char('c'));

    assert!(paste(&mut app, "add a notify step").is_none());

    let screen = rendered(&mut app);
    assert!(
        !screen.contains("agent template ·"),
        "the popup does not follow the operator off its page: {screen}"
    );
    assert!(
        screen.contains("add a notify step"),
        "and the paste reached the composer it can see: {screen}"
    );
}

#[test]
fn a_modal_over_the_copilot_swallows_the_paste_it_is_covering() {
    // `/resume` answers asynchronously, so its picker opens over whichever
    // tab the operator has moved to in the meantime — the copilot included.
    // The keyboard gives the modal precedence; paste has to agree, or the
    // payload lands in a draft the modal is sitting on top of and gets
    // submitted once it closes.
    let home = tempfile::tempdir().expect("temp home");
    let mut app = workflows_app(home.path());
    key_press(&mut app, KeyCode::Char('c'));
    app.open_resume(vec![medulla_tui::ui::chat_store::MainChatSummary {
        session_id: "chat-1".into(),
        name: "yesterday".into(),
        turns: 3,
        thread_count: 1,
        updated_at: "2026-08-02T00:00:00Z".into(),
    }]);
    assert!(app.resume_open(), "the picker is over the copilot");

    assert!(paste(&mut app, "stray text").is_none());

    // Dismiss it and look at the composer it was covering.
    key_press(&mut app, KeyCode::Esc);
    assert!(!app.resume_open());
    let screen = rendered(&mut app);
    assert!(
        !screen.contains("stray text"),
        "the modal swallowed it rather than the draft behind it: {screen}"
    );
}
