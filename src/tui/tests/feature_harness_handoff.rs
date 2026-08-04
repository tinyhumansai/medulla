//! Handing a harness back *with a brief*.
//!
//! Kept apart from `feature_harness_control`, which is about who may dispatch
//! into a harness. This is about what the orchestrator is told when a person
//! stops working in one — the note they leave, the pane output that goes with
//! it, and which harness `/handoff` means when the cursor is somewhere else.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, Cmd, TABS};
use medulla_tui::ui::harness_pane::LocalHarnesses;
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn app_with_harnesses(sessions: PtyManager) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();

    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
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
        attribution: true,
        budget: None,
        custom_harnesses: Vec::new(),
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
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());

    app.set_local_harnesses(LocalHarnesses {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        sessions,
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn render(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
}

/// Walk the rail until a frame resolves a harness under the cursor.
fn select_harness_row(app: &mut App) {
    for _ in 0..64 {
        render(app);
        if app.harness_pane_session_for_test().is_some() {
            return;
        }
        let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    }
    panic!("never reached the harness row");
}

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

/// A running session the operator holds, painting one identifiable line.
fn user_session_in(sessions: &PtyManager, cwd: &str) -> String {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    let id = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: cwd.to_string(),
            env,
            extra_args: vec![
                "-c".to_string(),
                "printf 'applying the migration\\r\\n'; sleep 30".to_string(),
            ],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            user_spawned: true,
        })
        .expect("open");
    let painted = sessions.clone();
    let watch = id.clone();
    wait_for("the harness to paint", || {
        painted
            .tail_lines(&watch, 50)
            .iter()
            .any(|line| line.contains("applying the migration"))
    });
    id
}

/// The queued handoff brief, if the last event raised one.
fn queued_brief(app: &mut App) -> Option<medulla::hub::HarnessHandoff> {
    while let Some(cmd) = app.take_pending_cmd() {
        if let Cmd::HandOffHarness(brief) = cmd {
            return Some(*brief);
        }
    }
    None
}

#[test]
fn handoff_with_a_note_queues_a_brief_carrying_it() {
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/handoff migration is half applied, tests RED");
    let _ = app.on_event(key(KeyCode::Enter));

    let brief = queued_brief(&mut app).expect("a handback must queue a brief");
    assert_eq!(
        brief.note.as_deref(),
        Some("migration is half applied, tests RED"),
        "the operator's own words, capitalisation and all"
    );
    assert_eq!(brief.session_id, id);
    assert_eq!(brief.provider, "codex");
    assert_eq!(brief.workspace_path, "/");
    assert!(
        brief.transcript.contains("applying the migration"),
        "the brief must carry what was on the pane: {:?}",
        brief.transcript
    );
    // And the local flag moved, so dispatch can use it again immediately.
    assert_eq!(
        sessions.control(&id),
        Some(HarnessControl::Orchestrator),
        "control flips locally without waiting for the brief to send"
    );

    sessions.close(&id);
}

#[test]
fn handoff_without_a_note_still_queues_a_brief() {
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/handoff");
    let _ = app.on_event(key(KeyCode::Enter));

    let brief = queued_brief(&mut app).expect("a note is optional, a brief is not");
    assert_eq!(brief.note, None);
    assert!(brief.transcript.contains("applying the migration"));

    sessions.close(&id);
}

#[test]
fn handoff_resolves_the_held_harness_with_no_frame_rendered() {
    // The regression. `harness_pane_session` is written inside the Agents draw
    // and cleared at the top of every frame, so `/handoff` used to depend on
    // what the *previous* render happened to resolve — reporting "no harness on
    // this row" while the operator was plainly holding one, and throwing away
    // the note they had just typed with it.
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());
    assert!(
        app.harness_pane_session_for_test().is_none(),
        "this test is only about anything while no frame has resolved a harness"
    );

    type_str(&mut app, "/handoff take it from here");
    let _ = app.on_event(key(KeyCode::Enter));

    let brief = queued_brief(&mut app)
        .expect("the one harness the operator holds is not ambiguous, rendered or not");
    assert_eq!(brief.session_id, id);
    assert_eq!(brief.note.as_deref(), Some("take it from here"));

    sessions.close(&id);
}

#[test]
fn handoff_with_two_held_harnesses_asks_rather_than_guessing() {
    // Handing back the wrong one puts an agent into a workspace somebody is
    // still working in, so ambiguity is reported instead of resolved.
    let sessions = PtyManager::new();
    let first = user_session_in(&sessions, "/");
    let second = user_session_in(&sessions, "/tmp");
    let mut app = app_with_harnesses(sessions.clone());
    app.tab_index = TABS.iter().position(|t| *t == "Overview").unwrap();

    type_str(&mut app, "/handoff");
    let _ = app.on_event(key(KeyCode::Enter));

    assert!(
        queued_brief(&mut app).is_none(),
        "an ambiguous handoff must not pick a harness"
    );
    assert_eq!(
        sessions.control(&first),
        Some(HarnessControl::User),
        "neither harness may be handed back on a guess"
    );
    assert_eq!(sessions.control(&second), Some(HarnessControl::User));

    sessions.close(&first);
    sessions.close(&second);
}

#[test]
fn ctrl_g_handback_queues_a_brief_too() {
    // Every path into a handback funnels through the same choke point, so the
    // keyboard shortcut must carry a brief exactly as the command does.
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());
    // Ctrl-G is cursor-scoped, so put the cursor on the harness row first.
    select_harness_row(&mut app);

    let _ = app.on_event(ctrl('g'));

    let brief = queued_brief(&mut app).expect("Ctrl-G handback must send a brief");
    assert_eq!(brief.session_id, id);
    assert_eq!(brief.note, None, "the shortcut has nowhere to type a note");
    assert_eq!(sessions.control(&id), Some(HarnessControl::Orchestrator));

    sessions.close(&id);
}

#[test]
fn taking_a_harness_tells_the_orchestrator_the_workspace_is_occupied() {
    // The other direction of the same fact. Without this the backend keeps
    // dispatching into a workspace a person is sitting in and only finds out by
    // being refused, one task at a time.
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    sessions.set_control(&id, HarnessControl::Orchestrator);
    let mut app = app_with_harnesses(sessions.clone());
    select_harness_row(&mut app);

    let _ = app.on_event(ctrl('g'));

    let mut held = None;
    while let Some(cmd) = app.take_pending_cmd() {
        if let Cmd::HoldHarness { workspace, .. } = cmd {
            held = Some(workspace);
        }
    }
    assert_eq!(held.as_deref(), Some("/"));
    assert_eq!(sessions.control(&id), Some(HarnessControl::User));

    sessions.close(&id);
}

#[test]
fn the_ask_modal_takes_a_note_with_e_and_commits_it_on_enter() {
    // The moment the operator actually has the context is when they are leaving
    // the harness, so the release prompt is where it is worth asking. `E` opens
    // the note; `y` still answers the question.
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());
    select_harness_row(&mut app);

    // Attach, then release: attaching is what takes control, and releasing is
    // what asks the question.
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));

    let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        screen.contains("add a note"),
        "the release prompt must offer the note: {screen}"
    );
    // Said before it is sent, because it leaves the machine.
    assert!(
        screen.contains("recent output"),
        "the prompt must disclose that pane output is sent: {screen}"
    );

    let _ = app.on_event(key(KeyCode::Char('e')));
    type_str(&mut app, "no, the redirect assertion is the broken one");
    // Enter from the note commits both at once. `y` here would be a letter —
    // which is the whole reason the note is modal.
    let _ = app.on_event(key(KeyCode::Enter));

    let brief = queued_brief(&mut app).expect("committing the note must send a brief");
    assert_eq!(
        brief.note.as_deref(),
        Some("no, the redirect assertion is the broken one"),
        "a note beginning with \"no\" must not have its first letter answer the question"
    );

    sessions.close(&id);
}

#[test]
fn keeping_the_harness_sends_nothing() {
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());
    select_harness_row(&mut app);

    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    let _ = app.on_event(key(KeyCode::Char('n')));

    assert!(
        queued_brief(&mut app).is_none(),
        "keeping a harness is not a handoff"
    );
    assert_eq!(sessions.control(&id), Some(HarnessControl::User));

    sessions.close(&id);
}

#[test]
fn escape_leaves_the_note_intact_and_y_then_sends_it() {
    // Escape out of the note means "stop typing", not "discard my sentence" —
    // an operator who loses a paragraph to a stray key does not write another.
    let sessions = PtyManager::new();
    let id = user_session_in(&sessions, "/");
    let mut app = app_with_harnesses(sessions.clone());
    select_harness_row(&mut app);

    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));

    let _ = app.on_event(key(KeyCode::Char('e')));
    type_str(&mut app, "half applied");
    let _ = app.on_event(key(KeyCode::Esc));
    // Back at the question, so `y` answers it again.
    let _ = app.on_event(key(KeyCode::Char('y')));

    let brief = queued_brief(&mut app).expect("yes must still hand back");
    assert_eq!(brief.note.as_deref(), Some("half applied"));

    sessions.close(&id);
}
