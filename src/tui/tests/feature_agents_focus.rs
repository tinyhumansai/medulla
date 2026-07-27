//! Feature tests for Agents-tab focus: reaching the rail from the keyboard,
//! getting back to the composer, and clicking a row below the lanes.
//!
//! The regression these pin is that the rail was reachable only by `Alt`+arrow,
//! which a stock macOS terminal does not send — so on those machines nothing but
//! the orchestrator could ever be selected, from the keyboard or the mouse.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;

/// An app parked on the Agents tab with the demo runtime's lanes.
fn agents_app() -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = medulla_tui::ui::app::TABS
        .iter()
        .position(|t| *t == "Agents")
        .expect("Agents tab exists");
    app.refresh_snapshot();
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

/// Render once so the click hitboxes are populated.
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

fn click(app: &mut App, x: u16, y: u16) {
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn the_composer_holds_focus_when_the_tab_opens() {
    // Typing must work immediately — that is why chat lives on this tab.
    let mut app = agents_app();

    assert!(!app.agents_rail_focused());
    typed(&mut app, "hello");
    assert_eq!(app.draft_text(), "hello");
}

#[test]
fn esc_on_an_empty_composer_hands_the_keyboard_to_the_rail() {
    // The whole bug: without this there is no keystroke a stock macOS terminal
    // can send that reaches the rail.
    let mut app = agents_app();

    key(&mut app, KeyCode::Esc);

    assert!(app.agents_rail_focused());
}

#[test]
fn esc_clears_a_draft_before_it_moves_focus() {
    // Losing a half-typed message to a focus key would be a worse bug than the
    // one being fixed, so clearing takes precedence and focus needs a second Esc.
    let mut app = agents_app();
    typed(&mut app, "a message");

    key(&mut app, KeyCode::Esc);
    assert_eq!(app.draft_text(), "");
    assert!(!app.agents_rail_focused());

    key(&mut app, KeyCode::Esc);
    assert!(app.agents_rail_focused());
}

#[test]
fn bare_arrows_walk_the_rail_once_it_has_focus() {
    let mut app = agents_app();
    let start = app.agent_index();

    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);

    assert!(app.agents_rail_focused());
    assert_ne!(
        app.agent_index(),
        start,
        "Down should have moved the rail cursor off the orchestrator"
    );
}

#[test]
fn arrows_still_drive_the_caret_while_the_composer_has_focus() {
    // Focus is what makes the arrows unambiguous; the composer's own behaviour
    // must not change for anyone who never leaves it.
    let mut app = agents_app();
    let start = app.agent_index();
    typed(&mut app, "one\ntwo");

    key(&mut app, KeyCode::Up);

    assert_eq!(app.agent_index(), start, "the rail must not have moved");
    assert_eq!(app.draft_text(), "one\ntwo", "the draft must be intact");
}

#[test]
fn enter_returns_the_keyboard_to_the_composer_without_submitting() {
    let mut app = agents_app();
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);

    let cmd = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));

    assert!(cmd.is_none(), "Enter on the rail must not submit a turn");
    assert!(!app.agents_rail_focused());
}

#[test]
fn typing_from_the_rail_returns_to_the_composer_and_keeps_the_character() {
    // A user who stepped out to look at a lane and started typing should not
    // lose the first letters into a list that ignores them.
    let mut app = agents_app();
    key(&mut app, KeyCode::Esc);

    typed(&mut app, "hi");

    assert!(!app.agents_rail_focused());
    assert_eq!(app.draft_text(), "hi");
}

#[test]
fn the_rail_selection_survives_the_trip_back_to_the_composer() {
    // Selecting an agent and then typing is the point: the composer's caption
    // and any answer routing depend on the cursor staying where it was put.
    let mut app = agents_app();
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);
    let picked = app.agent_index();

    key(&mut app, KeyCode::Enter);
    typed(&mut app, "status?");

    assert_eq!(app.agent_index(), picked);
    assert_eq!(app.draft_text(), "status?");
}

#[test]
fn focus_is_visible_on_screen() {
    // Focus that cannot be seen is focus a user cannot trust; the caption names
    // the key that moves it in both directions.
    let mut app = agents_app();
    let composer = render(&mut app, 120, 40);
    assert!(
        composer.contains("Esc to pick an agent"),
        "composer should advertise the way out: {composer}"
    );

    key(&mut app, KeyCode::Esc);
    let rail = render(&mut app, 120, 40);
    assert!(
        rail.contains("Enter or type to write"),
        "the rail should advertise the way back: {rail}"
    );
    assert_ne!(composer, rail, "the two focus states should look different");
}

#[test]
fn clicking_a_row_selects_it_and_takes_focus() {
    // The click handler read the lane list while the rail rendered lanes *plus*
    // the fleet and template rows, so every click below the lanes indexed off
    // the end of the shorter list and silently did nothing.
    let mut app = agents_app();
    render(&mut app, 120, 40);
    let start = app.agent_index();

    // Row 2 of the rail's interior: past the panel border and the first row.
    click(&mut app, 3, 4);

    assert!(app.agents_rail_focused(), "a click should take focus");
    assert_ne!(app.agent_index(), start, "the click should have selected");
}
