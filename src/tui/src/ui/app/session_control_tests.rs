//! Focused tests for the session-control chords: what they classify as text,
//! what they refuse, and how the render pass arms the remote-session refusal in
//! the first place.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
#[cfg(unix)]
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::rail::RailRow;
use super::session_control::is_text_input;
use super::types::{tab_pos, App};
#[cfg(unix)]
use crate::worker::pty::SessionControl;

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

    app.toggle_session_control();

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
    // message must stay the plain one. A remote-session sentence on a row that
    // names no session would be worse than the generic answer.
    let mut app = app();
    hosting(&mut app);
    app.pane_session = None;
    app.pane_remote_session = None;

    app.toggle_session_control();

    let status = app.status().to_string();
    assert!(
        status.contains("not holding any session"),
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
    let rows = app.rail_rows();
    rows.iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("no such row on the rail: {rows:?}"))
}

/// Select a rail row through the cursor API so its stable anchor follows it.
fn select_row(app: &mut App, wanted: impl Fn(&RailRow) -> bool) {
    app.set_rail_cursor(row_index(app, wanted));
}

#[test]
fn selecting_a_session_this_device_does_not_host_arms_the_remote_refusal() {
    // The other half of `taking_a_session_on_another_host_is_refused_by_name`:
    // that test sets `pane_remote_session` by hand, so nothing pinned the render
    // pass that is supposed to set it. Nothing on this device hosts, so every
    // session the demo fixture dispatched belongs to another machine — and the
    // cursor landing on one is the whole of what arms the refusal.
    let mut app = app();
    app.tab_index = tab_pos("Sessions");
    select_row(&mut app, |row| matches!(row, RailRow::Session(_)));

    draw_once(&mut app);

    let armed = app
        .pane_remote_session
        .clone()
        .expect("a session row this device does not run is a remote session");
    assert!(app.pane_session.is_none(), "and it is not a local one");

    // And the chord reads what the draw recorded, rather than the generic
    // "no session on this row" that an unarmed pointer would produce.
    app.toggle_session_control();
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
    // Hosting, so the rail carries the action row as well as the dispatched
    // session — the test needs a second row to move the cursor onto.
    app.set_local_sessions(super::rail::tests::shell_harnesses(
        crate::worker::pty::PtyManager::new(),
    ));
    app.tab_index = tab_pos("Sessions");
    select_row(&mut app, |row| matches!(row, RailRow::Session(_)));
    draw_once(&mut app);
    assert!(app.pane_remote_session.is_some(), "armed to begin with");

    // Off the row: the action row is not a session at all.
    select_row(&mut app, |row| matches!(row, RailRow::NewSession));
    draw_once(&mut app);
    assert!(
        app.pane_remote_session.is_none(),
        "the action row names no session: {:?}",
        app.pane_remote_session
    );

    // Off the tab: nothing on Settings draws the rail, so nothing re-arms it.
    select_row(&mut app, |row| matches!(row, RailRow::Session(_)));
    draw_once(&mut app);
    assert!(app.pane_remote_session.is_some(), "armed again");
    app.tab_index = tab_pos("Settings");
    draw_once(&mut app);
    assert!(
        app.pane_remote_session.is_none(),
        "a tab that never drew the row must not answer for it: {:?}",
        app.pane_remote_session
    );

    app.toggle_session_control();
    let status = app.status().to_string();
    assert!(
        !status.contains("another host"),
        "so the chord falls back to the plain answer: {status}"
    );
}

#[test]
fn an_unbound_character_in_a_harness_diff_does_not_type_into_the_hidden_draft() {
    let mut app = app();
    app.pane_session = Some("session-a".to_string());
    app.pane_view = super::types::PaneView::Diff;

    let _ = app.on_sessions_rail_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert!(
        app.draft.text.is_empty(),
        "the diff has no visible composer to receive this character"
    );
}

// Unix-only because the fixture uses `/bin/sh` to stand up real, local
// harnesses. The ownership behaviour itself is platform-independent.
#[cfg(unix)]
#[test]
fn closing_one_of_two_taken_sessions_keeps_the_shared_workspace_held() {
    let sessions = crate::worker::pty::PtyManager::new();
    let mut app = app();
    app.set_local_sessions(super::rail::tests::shell_harnesses(sessions.clone()));
    // Two sessions in one directory, opened the way the picker opens them:
    // the workspace hold is per-directory, so both have to name the same one.
    let choice = crate::ui::harness_pane::HarnessChoice::native(HarnessProvider::Codex);
    app.spawn_session(choice.clone(), "/");
    app.spawn_session(choice, "/");
    let ids: Vec<String> = sessions.rows().into_iter().map(|row| row.id).collect();
    assert_eq!(ids.len(), 2, "the fixture opened both sessions");
    for id in &ids {
        sessions.set_control(id, SessionControl::User);
        app.sessions_taken
            .insert(id.clone(), super::types::TakeOrigin::Explicit);
    }

    app.close_session(&ids[0]);

    assert!(
        app.sessions_taken.contains_key(&ids[1]),
        "the other session remains operator-held"
    );
    assert!(
        app.pending_cmds
            .iter()
            .all(|cmd| !matches!(cmd, super::types::Cmd::HandOffSession(_))),
        "the workspace hold stays in place while another taken session covers it"
    );
    sessions.shutdown();
}

// Unix-only because the fixture stands a real child up on a real pseudo-terminal.
#[cfg(unix)]
#[test]
fn uppercase_k_kills_a_local_session_that_has_no_dispatched_task() {
    // An operator-started harness has no task record, so it cannot be sent
    // through the remote task-kill protocol. `K` must still end the local
    // process after its destructive confirmation.
    let sessions = crate::worker::pty::PtyManager::new();
    let mut app = app();
    app.set_local_sessions(super::rail::tests::shell_harnesses(sessions.clone()));
    let choice = crate::ui::harness_pane::HarnessChoice::native(HarnessProvider::Codex);
    app.spawn_session(choice, "/");
    let id = sessions
        .rows()
        .into_iter()
        .next()
        .expect("a session was opened")
        .id;
    app.pane_session = Some(id.clone());

    let action = app.on_sessions_rail_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
    assert!(matches!(action, super::keys::SessionsKey::Handled(None)));
    assert_eq!(app.harness_close_armed.as_deref(), Some(id.as_str()));

    app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(
        !sessions
            .row(&id)
            .expect("the closed row is retained")
            .state
            .is_running(),
        "confirmation should kill the local harness"
    );
}

#[test]
fn local_session_kill_confirmation_is_drawn_as_a_modal() {
    let mut app = app();
    app.arm_harness_close("session-a".into());
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| app.draw(frame)).expect("draw modal");
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Kill this session?"), "{rendered}");
    assert!(rendered.contains("[Y] kill session"), "{rendered}");
}

// Unix-only because the fixture stands a real child up on a real pseudo-terminal
// via `/bin/sh`. The note plumbing itself is platform-independent.
#[cfg(unix)]
#[test]
fn the_handback_note_editor_puts_the_operators_words_on_the_brief() {
    // The note used to come from `/handoff <words>` typed into the orchestrator's
    // composer. That composer went with the orchestrator, so the hand-back
    // question's own editor is the one place an operator still writes one — and
    // it has to reach the brief exactly as the command did.
    let sessions = crate::worker::pty::PtyManager::new();
    let mut app = app();
    app.set_local_sessions(super::rail::tests::shell_harnesses(sessions.clone()));
    let choice = crate::ui::harness_pane::HarnessChoice::native(HarnessProvider::Codex);
    app.spawn_session(choice, "/");
    let id = sessions.rows().remove(0).id;
    sessions.set_control(&id, SessionControl::User);

    app.handback_prompt = Some(super::types::HandbackPrompt {
        session: id.clone(),
        took_control: false,
        note: Default::default(),
        editing_note: false,
        is_takeover: false,
    });

    // `e` opens the editor, the words go in, Enter sends the note with the brief.
    app.handle_handback_key(KeyCode::Char('e'));
    for character in "tests RED".chars() {
        app.handle_handback_key(KeyCode::Char(character));
    }
    app.handle_handback_key(KeyCode::Enter);

    let brief = app
        .pending_cmds
        .iter()
        .find_map(|cmd| match cmd {
            super::types::Cmd::HandOffSession(brief) => Some(brief.clone()),
            _ => None,
        })
        .expect("answering the question queues a brief");
    assert_eq!(brief.session_id, id);
    assert_eq!(
        brief.note.as_deref(),
        Some("tests RED"),
        "the operator's own words, capitalisation and all"
    );
    assert!(
        app.handback_prompt.is_none(),
        "and the question closes behind it"
    );

    sessions.shutdown();
}

/// A shell is the operator's, permanently. Handing one to the orchestrator
/// would advertise a row as dispatchable that no task can reach — a task frame
/// naming a shell is refused at the wire parse — and would queue a handoff
/// brief summarising a terminal.
// Unix-only for the same reason its neighbours are: the fixture stands a real
// child up on a real pseudo-terminal.
#[cfg(unix)]
#[test]
fn a_shell_session_cannot_be_handed_to_the_orchestrator() {
    let sessions = crate::worker::pty::PtyManager::new();
    let mut app = app();
    app.set_local_sessions(super::rail::tests::shell_harnesses(sessions.clone()));
    let choice =
        crate::ui::harness_pane::HarnessChoice::shell(crate::ui::harness_pane::ShellChoice {
            name: "sh".to_string(),
            bin: "/bin/sh".to_string(),
        });
    app.spawn_session(choice, "/");
    let id = sessions.rows().remove(0).id;

    app.hand_back_session(&id, None);

    assert_eq!(
        sessions.control(&id),
        Some(SessionControl::User),
        "the shell stays with the operator"
    );
    assert!(
        app.pending_cmds
            .iter()
            .all(|cmd| !matches!(cmd, super::types::Cmd::HandOffSession(_))),
        "and no brief is queued for it"
    );
    sessions.shutdown();
}
