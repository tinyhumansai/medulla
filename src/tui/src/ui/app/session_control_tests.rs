//! Focused tests for the session-control chords: what they classify as text,
//! and what they refuse.

use std::sync::Arc;

use crossterm::event::KeyModifiers;
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use super::session_control::is_text_input;
use super::types::App;

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
