//! Starting a harness the orchestrator will not touch, and moving control of
//! one between the operator and the orchestrator.
//!
//! The load-bearing assertion in most of these is not what is on screen but what
//! `claim_idle` will hand out: the badge is only a report, and a badge that said
//! "unmanaged" over a session dispatch would still reuse is the exact failure
//! worth testing for.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::tinyplace::HarnessProvider;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::harness_pane::LocalHarnesses;
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};

/// An Agents-tab app with a live session manager behind it.
///
/// `providers` is Codex alone: its interactive argv is empty, so the picker can
/// launch `/bin/sh` through the ordinary path without claude's minted
/// `--session-id` reaching a shell that would reject it.
fn app_with_harnesses(sessions: PtyManager) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();

    let config = medulla::daemon::DaemonConfig {
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/".to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        budget: None,
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        Arc::new(|_| Box::pin(async { Err("not used in these tests".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // The picker resolves the binary through `provider_bin`, which reads this
    // override — so "start codex" starts a shell that sits there instead.
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());

    app.set_local_harnesses(LocalHarnesses {
        sessions,
        runtime: medulla::daemon::DaemonRuntime::new(config, run_task, send),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        router: None,
    });
    app
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// The focus chord, as a terminal actually delivers it.
fn focus_chord() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL))
}

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// Spin until `check` passes; children on real ptys are at the mercy of load.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// A spec for a session the operator started, opened directly.
fn user_session(sessions: &PtyManager) -> String {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: "/".to_string(),
            env,
            extra_args: vec!["-c".to_string(), "sleep 30".to_string()],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            user_spawned: true,
        })
        .expect("open")
}

#[test]
fn ctrl_t_opens_the_picker_and_enter_starts_an_unmanaged_harness() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let out = render(&mut app, 140, 44);
    assert!(out.contains("Start a harness"), "{out}");
    assert!(
        out.contains("the orchestrator will not dispatch into it"),
        "the picker must say what unmanaged means: {out}"
    );

    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);

    let row = sessions.rows().remove(0);
    assert!(row.user_spawned);
    assert_eq!(row.control, HarnessControl::User);
    assert!(!row.busy, "nothing is running in it yet");
    assert!(
        app.status().contains("unmanaged"),
        "the operator is told what they just started: {}",
        app.status()
    );

    sessions.shutdown();
}

#[test]
fn an_unmanaged_harness_gets_its_own_rail_row() {
    // Lanes come from task events, so a session nothing dispatched into folds to
    // no lane at all. Without its own group it would be running and invisible.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let _ = user_session(&sessions);

    let out = render(&mut app, 140, 44);
    assert!(out.contains("your harnesses"), "{out}");
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
    assert!(!out.contains("Start a harness"), "{out}");
    assert!(sessions.rows().is_empty(), "Esc must not start a harness");
}

#[test]
fn the_picker_directory_can_be_edited_before_starting() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    let _ = app.on_event(ctrl('t'));
    let _ = app.on_event(key(KeyCode::Char('e')));
    type_str(&mut app, "tmp");
    let _ = app.on_event(key(KeyCode::Enter));

    let out = render(&mut app, 140, 44);
    assert!(out.contains("/tmp"), "the picker must show the edited path: {out}");
    assert!(sessions.rows().is_empty(), "editing must not start the harness");

    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);
    assert_eq!(sessions.rows().remove(0).cwd, "/tmp");

    sessions.shutdown();
}

#[test]
fn ctrl_g_hands_a_harness_over_and_back() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);

    // Put the cursor on the harness row; the pane resolves it on that draw.
    let _ = render(&mut app, 140, 44);
    let rows = sessions.rows().len();
    assert_eq!(rows, 1);
    select_harness_row(&mut app);

    let _ = app.on_event(ctrl('g'));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        HarnessControl::Orchestrator,
        "give: {}",
        app.status()
    );
    assert!(app.status().contains("Handed back"), "{}", app.status());
    // …and now dispatch may reuse it, which is the point of handing it over.
    assert!(sessions
        .claim_idle("you:codex", HarnessProvider::Codex)
        .is_some());
    sessions.release(&id);

    let _ = app.on_event(ctrl('g'));
    assert_eq!(sessions.row(&id).unwrap().control, HarnessControl::User);
    assert!(
        sessions
            .claim_idle("you:codex", HarnessProvider::Codex)
            .is_none(),
        "taking it back must close dispatch off again"
    );

    sessions.shutdown();
}

#[test]
fn attaching_takes_control_and_releasing_asks_for_it_back() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    // Start it under the orchestrator, so attaching is what takes it.
    sessions.set_control(&id, HarnessControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);

    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_harness(), Some(id.as_str()));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        HarnessControl::User,
        "focusing in takes the harness: {}",
        app.status()
    );

    // Releasing asks rather than deciding.
    let _ = app.on_event(focus_chord());
    let out = render(&mut app, 140, 44);
    assert!(out.contains("You still have this harness"), "{out}");
    assert_eq!(
        app.attached_harness(),
        Some(id.as_str()),
        "the keyboard stays put until the question is answered"
    );

    let _ = app.on_event(key(KeyCode::Char('y')));
    assert_eq!(app.attached_harness(), None);
    assert_eq!(
        sessions.row(&id).unwrap().control,
        HarnessControl::Orchestrator
    );

    sessions.shutdown();
}

#[test]
fn keeping_a_harness_releases_the_keyboard_but_not_control() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, HarnessControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());

    let _ = app.on_event(key(KeyCode::Char('n')));
    assert_eq!(app.attached_harness(), None, "the keyboard comes back");
    assert_eq!(
        sessions.row(&id).unwrap().control,
        HarnessControl::User,
        "but the harness does not"
    );
    assert!(app.status().contains("still hold"), "{}", app.status());

    sessions.shutdown();
}

#[test]
fn escape_from_the_question_stays_attached() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, HarnessControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(key(KeyCode::Esc));

    assert_eq!(
        app.attached_harness(),
        Some(id.as_str()),
        "Esc is 'I did not mean to leave'"
    );
    assert_eq!(sessions.row(&id).unwrap().control, HarnessControl::User);
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("You still have this harness"), "{out}");

    sessions.shutdown();
}

#[test]
fn the_slash_command_starts_a_named_provider_directly() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness codex /");
    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);

    let row = sessions.rows().remove(0);
    assert_eq!(row.control, HarnessControl::User);
    assert_eq!(row.cwd, "/");

    sessions.shutdown();
}

#[test]
fn a_misspelled_provider_reports_usage_rather_than_starting_one() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness claud");
    let _ = app.on_event(key(KeyCode::Enter));

    assert!(app.status().contains("Usage: /harness"), "{}", app.status());
    assert!(sessions.rows().is_empty(), "nothing should have started");
}

#[test]
fn starting_a_harness_in_a_missing_directory_says_so() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness codex /no/such/place");
    let _ = app.on_event(key(KeyCode::Enter));

    assert!(
        app.status().contains("not a directory"),
        "a bad path is named, not swallowed: {}",
        app.status()
    );
    assert!(sessions.rows().is_empty());
}

/// Walk the rail cursor onto the operator's harness row.
///
/// The row sits below every lane, so the count is not fixed — stepping down
/// until the pane resolves a session is what makes this independent of how many
/// lanes the mock runtime happens to produce.
fn select_harness_row(app: &mut App) {
    for _ in 0..64 {
        let _ = render(app, 140, 44);
        if app.harness_pane_session_for_test().is_some() {
            return;
        }
        let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    }
    panic!("never reached the harness row");
}
