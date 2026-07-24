//! Prepared-decision badge, overlay, dismissal, and answer routing.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::harness_contract::{
    HarnessState, HarnessStatus, HarnessUsage, TrackedTask, TrackedTaskStatus,
};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;
use medulla_tui::ui::events::{EventEnvelope, TuiEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn key(app: &mut App, code: KeyCode) {
    let _ = app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 42)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn app() -> (App, Arc<MockRuntime>) {
    let runtime = Arc::new(MockRuntime::empty());
    let mut app = App::new(
        runtime.clone(),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    app.snapshot.events = vec![
        EventEnvelope {
            seq: 1,
            at: 1,
            event: TuiEvent::TaskStart {
                task_id: "cycle-1/t:task-1".into(),
                instruction: "choose the schema".into(),
                depth: 1,
                agent_id: Some("dev".into()),
                contract: None,
            },
        },
        EventEnvelope {
            seq: 2,
            at: 2,
            event: TuiEvent::TaskAttention {
                task_id: "cycle-1/t:task-1".into(),
                reason: "confirm".into(),
                content: "use schema v2?".into(),
                question_id: Some("q1".into()),
            },
        },
    ];
    app.snapshot.harness = Some(HarnessStatus {
        state: HarnessState::Running,
        queued: 0,
        active_instruction_id: None,
        active_cycle_id: Some("cycle-1".into()),
        tasks: vec![TrackedTask {
            id: "task-1".into(),
            title: "Choose schema".into(),
            detail: Some("Preserve the public API".into()),
            status: TrackedTaskStatus::Blocked,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:01Z".into(),
            instruction_id: None,
            delegated_task_ids: vec!["cycle-1/t:task-1".into()],
            notes: vec![],
            contract: None,
            evidence: None,
        }],
        running_delegations: 1,
        usage: HarnessUsage::default(),
        last_result: None,
        escalations: vec!["Needs release approval".into()],
    });
    (app, runtime)
}

#[test]
fn overview_badge_opens_prepared_context_and_answer_routes_to_runtime() {
    let (mut app, runtime) = app();
    let overview = render(&mut app);
    assert!(overview.contains("decisions: 2 · E open"), "{overview}");

    key(&mut app, KeyCode::Char('E'));
    let overlay = render(&mut app);
    assert!(overlay.contains("Decisions · 1/2"), "{overlay}");
    assert!(overlay.contains("confirm: use schema v2?"), "{overlay}");
    assert!(overlay.contains("Preserve the public API"), "{overlay}");

    key(&mut app, KeyCode::Enter);
    for ch in "yes".chars() {
        key(&mut app, KeyCode::Char(ch));
    }
    key(&mut app, KeyCode::Enter);

    assert_eq!(app.status(), "Decision answered");
    assert_eq!(app.decisions().len(), 1);
    assert!(runtime
        .recorded_calls()
        .iter()
        .any(|call| call.contains("answer_question:cycle-1:q1:yes")));
}

#[test]
fn informational_escalations_dismiss_and_empty_queue_reports_cleanly() {
    let (mut app, _) = app();
    key(&mut app, KeyCode::Char('E'));
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Down);
    let overlay = render(&mut app);
    assert!(overlay.contains("Needs release approval"), "{overlay}");
    assert!(overlay.contains("informational escalation"), "{overlay}");

    key(&mut app, KeyCode::Enter);
    assert_eq!(app.decisions().len(), 1);

    // Remove the remaining worker question at its source while the modal is
    // open, then prove stale input safely closes it and E reports an empty queue.
    app.snapshot.events.clear();
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('E'));
    assert_eq!(app.status(), "No prepared decisions");
}
