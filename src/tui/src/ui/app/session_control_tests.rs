//! Focused tests for the session-control chords: what they classify as text,
//! what they refuse, and how the render pass arms the remote-session refusal in
//! the first place.

use std::sync::Arc;

use crossterm::event::KeyModifiers;
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::rail::RailRow;
use super::session_control::is_text_input;
use super::types::{tab_pos, App};

#[test]
fn workspace_text_accepts_altgr_but_rejects_control_shortcuts() {
    assert!(is_text_input(KeyModifiers::NONE));
    assert!(is_text_input(KeyModifiers::SHIFT));
    assert!(is_text_input(KeyModifiers::CONTROL | KeyModifiers::ALT));
    assert!(!is_text_input(KeyModifiers::CONTROL));
    assert!(!is_text_input(KeyModifiers::ALT));
}

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    App::new(rt, loaded)
}

#[test]
fn taking_a_session_on_another_host_is_refused_by_name() {
    // §E7. The hub resolves a hold by *local workspace path*, so there is
    // nothing on this machine to flip for a session running on another one —
    // the take would silently do nothing. Remote takeover needs the owner →
    // machine control frames wired into the hold path, and is a documented
    // follow-up (§G).
    //
    // The refusal has to name the machine. Both "the cursor is on nothing" and
    // "the cursor is on someone else's session" leave `pane_session` empty, and
    // an operator told "no session on this row" while plainly looking at one
    // reads it as a broken feature rather than as a boundary.
    let mut app = app();
    app.pane_session = None;
    app.pane_remote_session = Some("mac-studio-claude".to_string());

    app.take_session_control();

    let status = app.status().to_string();
    assert!(
        status.contains("mac-studio-claude") && status.contains("another host"),
        "the refusal must name the agent and say why: {status}"
    );
    assert!(
        status.contains("watch"),
        "and must say what the operator CAN do — a remote session is viewable, \
         which is the whole of the screen mirror: {status}"
    );
}

/// A hosting device with no sessions running on it.
fn hosting(app: &mut App) {
    app.local_sessions = Some(crate::ui::harness_pane::LocalSessions {
        sessions: crate::worker::pty::PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "this-device".to_string(),
        env: std::collections::HashMap::new(),
        workspace: "/repos/acme".to_string(),
        providers: Vec::new(),
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
    });
}

#[test]
fn the_take_chord_on_an_empty_row_still_says_so() {
    // The other side of the same branch: with no remote row recorded, the
    // message must stay the plain one. A remote-session sentence on a host row
    // or the composer would be worse than the generic answer.
    let mut app = app();
    hosting(&mut app);
    app.pane_session = None;
    app.pane_remote_session = None;

    app.take_session_control();

    let status = app.status().to_string();
    assert!(
        status.contains("No session on this row"),
        "unexpected status: {status}"
    );
}

/// Draw one whole frame, which is what records the pane pointers.
fn draw_once(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("a test terminal");
    terminal.draw(|frame| app.draw(frame)).expect("a frame");
}

/// The rail index of the first row satisfying `wanted`.
fn row_index(app: &App, wanted: impl Fn(&RailRow) -> bool) -> usize {
    app.rail_rows()
        .iter()
        .position(wanted)
        .expect("the demo fixture has such a row")
}

#[test]
fn selecting_a_session_this_device_does_not_host_arms_the_remote_refusal() {
    // The other half of `taking_a_session_on_another_host_is_refused_by_name`:
    // that test sets `pane_remote_session` by hand, so nothing pinned the render
    // pass that is supposed to set it. Nothing on this device hosts, so every
    // session the demo fixture dispatched belongs to another machine — and the
    // cursor landing on one is the whole of what arms the refusal.
    let mut app = app();
    app.tab_index = tab_pos("Agents");
    app.agent_index = row_index(&app, |row| matches!(row, RailRow::Session(_)));

    draw_once(&mut app);

    let armed = app
        .pane_remote_session
        .clone()
        .expect("a session row this device does not run is a remote session");
    assert!(app.pane_session.is_none(), "and it is not a local one");

    // And the chord reads what the draw recorded, rather than the generic
    // "no session on this row" that an unarmed pointer would produce.
    app.take_session_control();
    let status = app.status().to_string();
    assert!(
        status.contains(&armed) && status.contains("another host"),
        "the refusal names what the draw armed: {status}"
    );
}

#[test]
fn moving_off_the_row_or_off_the_tab_disarms_it_again() {
    // A pointer is only true for the frame that recorded it. Left standing, it
    // answers the take chord for a row — or a whole tab — the operator is no
    // longer looking at, which is how `Ctrl-G` on Settings ends up talking about
    // somebody else's machine.
    let mut app = app();
    app.tab_index = tab_pos("Agents");
    app.agent_index = row_index(&app, |row| matches!(row, RailRow::Session(_)));
    draw_once(&mut app);
    assert!(app.pane_remote_session.is_some(), "armed to begin with");

    // Off the row: the conversation is not a session at all.
    app.agent_index = row_index(&app, |row| matches!(row, RailRow::Lane(_)));
    draw_once(&mut app);
    assert!(
        app.pane_remote_session.is_none(),
        "a lane row names no session: {:?}",
        app.pane_remote_session
    );

    // Off the tab: nothing on Settings draws the rail, so nothing re-arms it.
    app.agent_index = row_index(&app, |row| matches!(row, RailRow::Session(_)));
    draw_once(&mut app);
    assert!(app.pane_remote_session.is_some(), "armed again");
    app.tab_index = tab_pos("Settings");
    draw_once(&mut app);
    assert!(
        app.pane_remote_session.is_none(),
        "a tab that never drew the row must not answer for it: {:?}",
        app.pane_remote_session
    );

    app.take_session_control();
    let status = app.status().to_string();
    assert!(
        !status.contains("another host"),
        "so the chord falls back to the plain answer: {status}"
    );
}
