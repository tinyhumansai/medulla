//! The pointer while a harness holds the keyboard: who owns a gesture that
//! starts in the embedded pane, and what a click that leaves one settles.
//!
//! Both halves of this file are about the same failure told from two sides. A
//! click out of an attached harness raises the hand-back question, and that
//! question used to arrive wrong in two ways at once — it swallowed the release
//! of a press the child was still holding, and when the click landed on the
//! rail it named the wrong harness while the next draw quietly detached the
//! right one.
//!
//! The child is a real `/bin/sh` on a real pty that turns mouse reporting on and
//! echoes what it receives with `cat -v`, so the assertions read the actual
//! bytes that reached it rather than a record of what we meant to send.
//!
//! Unix-only: it runs `/bin/sh` on a pty, which Windows has no equivalent of.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::harness_pane::LocalHarnesses;
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};

/// The terminal the assertions are written against.
///
/// Wide enough that the rail and the pane are both comfortably sized, and tall
/// enough that two harness rows are on screen at once — the second row is what
/// the rail half of this file clicks on.
const WIDTH: u16 = 140;
/// Rows of that terminal.
const HEIGHT: u16 = 44;

/// An Agents-tab app with a live session manager behind it.
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
        Arc::new(|_| Box::pin(async { Err("these tests dispatch nothing".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());

    app.set_local_harnesses(LocalHarnesses {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
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

/// A harness the operator holds, whose child asks for the mouse and echoes it.
///
/// `stty raw -echo` first so the pty stops rewriting what arrives; the DECSET
/// pair is what makes [`LocalHarnesses::takes_mouse`] true; `cat -v` renders the
/// reports it receives as printable text, which is what the pane snapshot can
/// then be searched for.
fn mouse_reporting_session(sessions: &PtyManager) -> String {
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
            extra_args: vec![
                "-c".to_string(),
                "stty raw -echo; printf 'READY\\r\\n\\033[?1000h\\033[?1006h'; cat -v".to_string(),
            ],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            origin: medulla_tui::worker::pty::SessionOrigin::User,
            name: None,
            mcp_grant_session: None,
        })
        .expect("open a session")
}

/// The harness's whole screen as one string.
fn screen(harnesses: &LocalHarnesses, id: &str) -> String {
    harnesses
        .screen(id)
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| row.iter().map(|c| c.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Spin until `check` passes; children on real ptys are at the mercy of load.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// Draw a frame and return its rows, so a test can find the row it means to
/// click rather than hardcoding a layout it does not control.
fn render(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| (0..WIDTH).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn pointer(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn press(column: u16, row: u16) -> Event {
    pointer(MouseEventKind::Down(MouseButton::Left), column, row)
}

fn release(column: u16, row: u16) -> Event {
    pointer(MouseEventKind::Up(MouseButton::Left), column, row)
}

/// Walk the rail to the first harness row and take the keyboard into it.
///
/// Returns the session the pane resolved. The cursor is moved with the arrows
/// and a frame is drawn after each step because `harness_pane_session` is
/// written during the draw — this is the same sequence an operator performs, and
/// the only one that leaves the pane rect recorded for the pointer to use.
fn attach_to_first_harness(app: &mut App) -> String {
    render(app);
    // Esc takes focus off the composer and puts it on the rail.
    let _ = app.on_event(key(KeyCode::Esc));
    render(app);
    for _ in 0..16 {
        if app.harness_pane_session_for_test().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
        render(app);
    }
    let session = app
        .harness_pane_session_for_test()
        .expect("the rail cursor to reach a harness row")
        .to_string();
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    render(app);
    assert_eq!(
        app.attached_harness(),
        Some(session.as_str()),
        "the chord must attach before the pointer cases begin"
    );
    session
}

/// Where an answer's label starts on the hand-back question's answer line.
///
/// Found by searching the drawn frame rather than by recomputing the layout, so
/// the tests click what an operator would actually be aiming at.
fn answer_at(lines: &[String], label: &str) -> (u16, u16) {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte) = line.find(label) {
            // Cells, not bytes: the frame is full of box-drawing characters, so
            // a byte offset would point well to the right of the label.
            let column = line[..byte].chars().count() as u16;
            return (column, row as u16);
        }
    }
    panic!("no answer {label:?} on screen:\n{}", lines.join("\n"));
}

/// The rows a harness occupies on the rail, top to bottom.
fn harness_rail_rows(lines: &[String]) -> Vec<u16> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("codex · unmanaged"))
        .map(|(index, _)| index as u16)
        .collect()
}

#[test]
fn a_release_outside_the_pane_still_reaches_the_harness_that_took_the_press() {
    // A terminal grabs the pointer on press. Without that, dragging out of the
    // pane and letting go left the child holding a button nobody was pressing,
    // and every later motion read as a drag from a stale anchor — which is what
    // put its popups somewhere the operator had not clicked.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });
    wait_for("the child to ask for the mouse", || {
        harnesses.takes_mouse(&id)
    });

    attach_to_first_harness(&mut app);
    let (pane, _) = app
        .harness_pane_rect_for_test()
        .expect("the draw to record the pane");

    let _ = app.on_event(press(pane.x + 2, pane.y + 2));
    wait_for("the press to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;3;3M")
    });

    // Let go over the rail, well outside the pane.
    let _ = app.on_event(release(1, 1));
    wait_for("the release to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;1;1m")
    });

    let out = screen(&harnesses, &id);
    assert!(
        out.contains("^[[<0;3;3M^[[<0;1;1m"),
        "the release must follow its own press, clamped into the pane: {out}"
    );

    sessions.shutdown();
}

#[test]
fn the_handback_question_does_not_swallow_the_release_of_a_press_it_interrupted() {
    // The reported sequence exactly: press inside the harness, click out, and
    // answer the question that opens. The click-out is a fresh press the modal
    // is entitled to swallow — the release of the *earlier* press is not, and
    // swallowing it is what left the child mid-drag behind the prompt.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });
    wait_for("the child to ask for the mouse", || {
        harnesses.takes_mouse(&id)
    });

    attach_to_first_harness(&mut app);
    let (pane, _) = app
        .harness_pane_rect_for_test()
        .expect("the draw to record the pane");

    let _ = app.on_event(press(pane.x + 4, pane.y + 1));
    wait_for("the press to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;5;2M")
    });

    // Click out onto the tab strip. This opens the hand-back question, which
    // owns the pointer from here on.
    let _ = app.on_event(press(3, 1));
    let out = render(&mut app).join("\n");
    assert!(
        out.contains("You still have this harness"),
        "clicking out must raise the hand-back question: {out}"
    );

    // The button the operator was still holding now comes up.
    let _ = app.on_event(release(3, 1));
    wait_for("the interrupted release to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;1;1m")
    });

    // The click that opened the question was never forwarded — only the release
    // of the press that preceded it.
    let out = screen(&harnesses, &id);
    assert_eq!(
        out.matches("^[[<0;").count(),
        2,
        "exactly the press and its own release reached the child: {out}"
    );

    sessions.shutdown();
}

#[test]
fn clicking_another_harness_row_asks_about_the_harness_being_left() {
    // The rail used to be waved through as navigation chrome, so a click on the
    // neighbouring harness row skipped the hand-back policy entirely. The next
    // draw then detached the attached session silently — while the question on
    // screen named the harness that had just been *clicked*. Answering it handed
    // back a session the operator had never typed in and left the one they had
    // held locked to them forever.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let held = mouse_reporting_session(&sessions);
    let other = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("both children to paint", || {
        screen(&harnesses, &held).contains("READY") && screen(&harnesses, &other).contains("READY")
    });

    let attached = attach_to_first_harness(&mut app);
    assert_eq!(attached, held, "the first rail row is the first session");

    let lines = render(&mut app);
    let rows = harness_rail_rows(&lines);
    assert_eq!(
        rows.len(),
        2,
        "both harnesses must be on the rail: {rows:?}"
    );

    let _ = app.on_event(press(5, rows[1]));
    assert_eq!(
        app.attached_harness(),
        Some(held.as_str()),
        "the click is not applied until the question is answered"
    );
    let out = render(&mut app).join("\n");
    assert!(
        out.contains("You still have this harness"),
        "leaving a harness by the rail must ask the same question: {out}"
    );
    assert_eq!(
        app.attached_harness(),
        Some(held.as_str()),
        "and the draw must not detach behind the question it is asking"
    );

    let _ = app.on_event(key(KeyCode::Char('y')));
    assert_eq!(
        harnesses.control(&held),
        Some(HarnessControl::Orchestrator),
        "yes hands back the harness the operator was actually in"
    );
    assert_eq!(
        harnesses.control(&other),
        Some(HarnessControl::User),
        "and never the one they merely clicked on"
    );
    assert_eq!(
        app.attached_harness(),
        None,
        "answering releases the keyboard"
    );

    sessions.shutdown();
}

#[test]
fn clicking_the_attached_harness_own_row_neither_asks_nor_releases() {
    // The row you are already typing in is not a destination away from it. This
    // is the carve-out the rail rule above narrows to, and it has to survive:
    // raising a handover question over a pane the operator is mid-sentence in
    // leaves them with nothing useful to press but Esc.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });

    let attached = attach_to_first_harness(&mut app);
    let lines = render(&mut app);
    let rows = harness_rail_rows(&lines);

    let _ = app.on_event(press(5, rows[0]));
    let out = render(&mut app).join("\n");
    assert!(
        !out.contains("You still have this harness"),
        "clicking your own row must ask nothing: {out}"
    );
    assert_eq!(
        app.attached_harness(),
        Some(attached.as_str()),
        "and must leave the keyboard where it was"
    );
    assert_eq!(
        harnesses.control(&attached),
        Some(HarnessControl::User),
        "and must not change who holds the harness"
    );

    sessions.shutdown();
}

#[test]
fn the_handback_question_is_answered_by_clicking_its_answers() {
    // The question is the one overlay a click can raise, so it is the one an
    // operator reaches with their hand already on the mouse. Each answer is a
    // target spanning its own label, and clicking it does exactly what pressing
    // the key in the bracket does.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });

    let attached = attach_to_first_harness(&mut app);
    let _ = app.on_event(press(3, 1));
    let lines = render(&mut app);
    assert!(
        lines.join("\n").contains("You still have this harness"),
        "the question must be up before it can be clicked"
    );

    // [E] opens the note field without answering the question.
    let (x, y) = answer_at(&lines, "[E] add a note");
    let _ = app.on_event(press(x + 1, y));
    let lines = render(&mut app);
    assert!(
        lines.join("\n").contains("Note: █"),
        "clicking [E] must start the note: {}",
        lines.join("\n")
    );
    assert_eq!(
        harnesses.control(&attached),
        Some(HarnessControl::User),
        "and must not answer the question on the way"
    );

    // [Esc] on the note line goes back to the question, still unanswered.
    let (x, y) = answer_at(&lines, "[Esc] back to the question");
    let _ = app.on_event(press(x + 1, y));
    let lines = render(&mut app);
    assert!(
        lines.join("\n").contains("[Y] hand back"),
        "clicking [Esc] must return to the answers: {}",
        lines.join("\n")
    );

    // [Y] hands the harness back, exactly as the key does.
    let (x, y) = answer_at(&lines, "[Y] hand back");
    let _ = app.on_event(press(x + 1, y));
    assert_eq!(
        harnesses.control(&attached),
        Some(HarnessControl::Orchestrator),
        "clicking [Y] must hand the harness back"
    );
    assert_eq!(app.attached_harness(), None, "and release the keyboard");
    assert!(
        !render(&mut app)
            .join("\n")
            .contains("You still have this harness"),
        "and close the question"
    );

    sessions.shutdown();
}

#[test]
fn a_click_on_the_question_that_is_not_an_answer_answers_nothing() {
    // The rest of the modal still swallows the pointer. The failure this guards
    // is the one that would follow from answering and then falling through: a
    // click that both closed the question and landed on the rail behind it.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_harnesses().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });

    let attached = attach_to_first_harness(&mut app);
    let _ = app.on_event(press(3, 1));
    let lines = render(&mut app);

    // The prose line above the answers is not a target.
    let (x, y) = answer_at(&lines, "While you hold it");
    let _ = app.on_event(press(x + 2, y));
    let out = render(&mut app).join("\n");
    assert!(
        out.contains("You still have this harness"),
        "a click on the body must leave the question up: {out}"
    );
    assert_eq!(
        harnesses.control(&attached),
        Some(HarnessControl::User),
        "and must answer nothing"
    );

    // Nor is the separator between two answers.
    let (x, y) = answer_at(&lines, "[Y] hand back");
    let _ = app.on_event(press(x + "[Y] hand back".chars().count() as u16 + 1, y));
    let out = render(&mut app).join("\n");
    assert!(
        out.contains("You still have this harness"),
        "a click between answers must leave the question up: {out}"
    );
    assert_eq!(
        harnesses.control(&attached),
        Some(HarnessControl::User),
        "and must answer nothing"
    );

    sessions.shutdown();
}
