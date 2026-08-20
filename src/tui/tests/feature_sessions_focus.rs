//! Feature tests for Agents-tab navigation: the rail holds the keyboard, bare
//! arrows walk it, and a click selects the row under the pointer.
//!
//! This file used to pin a focus *split* — the tab merged the orchestrator's
//! composer with the rail, one terminal keyboard drove both, and which of them
//! had it was a state the operator could neither see nor reliably move. There is
//! no composer here any more: the orchestrator is a subconscious layer rather
//! than something typed at, so the rail is the only thing on the tab that takes
//! keys. What is left to pin is that it always does, including on the rows that
//! used to hand the keyboard away.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;
use medulla_tui::ui::harness_pane::LocalSessions;
use medulla_tui::worker::pty::PtyManager;

/// An app parked on the Agents tab, hosting, with the demo runtime's sessions.
///
/// Hosting matters: the `+ New session` action is only offered on a device that
/// can start one, and without it the demo fixture's single dispatched session is
/// the whole rail — one row, which no arrow can move off.
fn agents_app() -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = medulla_tui::ui::app::TABS
        .iter()
        .position(|t| *t == "Sessions")
        .expect("Agents tab exists");
    app.set_local_sessions(LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions: PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "medulla-orchestrator".to_string(),
        env: std::collections::HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app.refresh_snapshot();
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
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

/// Render and return the screen row containing `needle`.
///
/// Session rows may wrap as the rail narrows, so mouse tests must click the row
/// they can actually see instead of assuming a fixed terminal line.
fn rendered_row(app: &mut App, w: u16, h: u16, needle: &str) -> u16 {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(w as usize)
        .position(|cells| {
            cells
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains(needle)
        })
        .map(|row| row as u16)
        .unwrap_or_else(|| panic!("missing rendered row containing {needle:?}"))
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
fn the_rail_holds_the_keyboard_when_the_tab_opens() {
    let app = agents_app();
    assert!(
        app.sessions_rail_focused(),
        "there is nothing else on the tab to hold it"
    );
}

#[test]
fn bare_arrows_walk_the_rail() {
    // The regression this pins predates the composer's removal: the rail was
    // reachable only by `Alt`+arrow, which a stock macOS terminal does not send,
    // so on those machines nothing could be selected from the keyboard at all.
    let mut app = agents_app();
    render(&mut app, 120, 40);
    let start = app.rail_index();

    key(&mut app, KeyCode::Down);

    assert_ne!(app.rail_index(), start, "↓ moves the rail cursor");

    key(&mut app, KeyCode::Up);

    assert_eq!(app.rail_index(), start, "and ↑ comes back");
}

#[test]
fn the_rail_keeps_the_keyboard_on_every_row() {
    // Printable keys used to hop the cursor to the orchestrator's lane and type
    // there. With no composer to type into, a character must leave the cursor
    // exactly where it was rather than moving it somewhere invisible.
    let mut app = agents_app();
    render(&mut app, 120, 40);
    key(&mut app, KeyCode::Down);
    let selected = app.rail_index();

    for character in "hello".chars() {
        key(&mut app, KeyCode::Char(character));
    }

    assert_eq!(
        app.rail_index(),
        selected,
        "typing must not move the selection"
    );
    assert!(
        app.sessions_rail_focused(),
        "and the rail still has the keys"
    );
}

#[test]
fn no_composer_is_drawn_on_the_tab() {
    // The pane is the whole right column now. A leftover text box would be the
    // worst of both worlds: visible, focusable by click, and wired to nothing.
    let mut app = agents_app();
    let out = render(&mut app, 120, 40);

    assert!(
        !out.contains("Esc to pick an agent") && !out.contains("Enter or type to write"),
        "the composer's focus captions must be gone: {out}"
    );
}

#[test]
fn clicking_a_row_selects_it() {
    // The click handler read the lane list while the rail rendered lanes *plus*
    // the fleet and template rows, so every click below the lanes indexed off
    // the end of the shorter list and silently did nothing.
    let mut app = agents_app();
    // Matched on the rail's own spelling: the pane beside it describes the same
    // row, and it draws wider, so a bare needle would find that line instead.
    let session_row = rendered_row(&mut app, 120, 40, "task-1");
    let start = app.rail_index();

    click(&mut app, 3, session_row);

    assert!(
        app.sessions_rail_focused(),
        "the rail still has the keyboard"
    );
    assert_ne!(app.rail_index(), start, "the click should have selected");
}
