//! Focused unit tests for the [`App`] screen: that every tab renders, the async
//! header toggle shows, and the composer/slash-command dispatch behaves.

mod harness_pane;

use super::*;
use std::sync::Arc;

use super::types::{PaneView, RP_HARNESSES};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let loaded = {
        let mut l = LoadedConfig::defaults("medulla.tui.json".into());
        l.config.link = Some(medulla::config::LinkConfig::default());
        l
    };
    App::new(rt, loaded)
}

fn app_with_running_task() -> App {
    let rt = MockRuntime::demo();
    let loaded = {
        let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
        loaded.config.link = Some(medulla::config::LinkConfig::default());
        loaded
    };
    let mut app = App::new(Arc::new(rt), loaded);
    app.snapshot.events.retain(|envelope| {
        !matches!(
            &envelope.event,
            crate::ui::events::TuiEvent::TaskComplete { digest }
                if digest.task_id == "task-1"
        )
    });
    app
}

/// The index of the tab named `name`. Looked up rather than written down: the
/// tab bar's order is a product decision that has changed before.
fn tab(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).expect("a known tab")
}

fn render(app: &mut App) -> String {
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect::<String>()
}

#[test]
fn every_tab_renders() {
    for (i, name) in TABS.iter().enumerate() {
        let mut a = app();
        a.tab_index = i;
        let out = render(&mut a);
        // The chrome, not the product name: the shortcut line heads every tab
        // and the status line closes it, so a tab that drew nothing fails here.
        assert!(
            out.contains("Tab views"),
            "tab {name} missing shortcut line"
        );
    }
}

#[test]
fn drawing_an_intervening_tab_preserves_the_harness_selected_for_changes() {
    let mut a = app();
    a.rail_session = Some("older-harness".to_owned());
    a.pane_session = Some("older-harness".to_owned());
    a.tab_index = tab("Workflows");

    render(&mut a);

    assert_eq!(a.pane_session, None, "hidden panes cannot receive keys");
    assert_eq!(
        a.rail_session.as_deref(),
        Some("older-harness"),
        "tab navigation must not discard the Changes repository selection"
    );
}

#[test]
fn harnesses_page_renders_custom_openrouter_presets_and_editor_controls() {
    let mut a = app();
    a.tab_index = tab("Hosts");
    a.routing_index = RP_HARNESSES;
    a.routing_focused = true;
    a.credential_status.openrouter_api_key = true;
    a.custom_harnesses = vec![medulla::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/deepseek-chat | | this-device",
    )
    .expect("valid custom harness")];

    let out = render(&mut a);

    assert!(out.contains("Custom OpenRouter harnesses"));
    assert!(out.contains("DeepSeek via Claude"));
    assert!(out.contains("key connected"));
    assert!(out.contains("a add"));
    assert!(out.contains("e edit"));
    assert!(!out.contains("sk-or-"));
}

#[test]
fn harnesses_page_marks_openhuman_presets_as_embedded_without_a_router_key() {
    let mut a = app();
    a.tab_index = tab("Hosts");
    a.routing_index = RP_HARNESSES;
    a.routing_focused = true;
    a.custom_harnesses = vec![medulla::config::CustomHarnessConfig::from_editor_line(
        "openhuman | OpenHuman | openhuman | some/model | | this-device",
    )
    .expect("valid OpenHuman custom harness")];

    let out = render(&mut a);

    assert!(out.contains("OpenHuman"));
    assert!(out.contains("embedded core"));
    assert!(!out.contains("key missing"));
}

#[test]
fn enter_answers_the_harness_picker_not_the_harness_behind_it() {
    use super::types::{SessionPicker, SessionPickerStep, WorkspaceChoice};
    use crate::ui::harness_pane::HarnessChoice;

    let mut a = app();
    a.tab_index = tab("Sessions");
    // The cursor is on a harness row, so the last frame recorded a session for
    // it — the state the attach shortcut reads. Opening the picker on top of
    // that used to lose the very next Enter to the pane underneath, which
    // attached instead of advancing to the workspace step.
    a.pane_session = Some("already-running".to_string());
    a.session_picker = Some(SessionPicker {
        choices: vec![HarnessChoice::native(
            medulla::protocol::HarnessProvider::Claude,
        )],
        index: 0,
        step: SessionPickerStep::Harness,
        cwd: ".".into(),
        workspace_query: String::new(),
        workspace_choices: Vec::new(),
        workspace_index: 0,
        workspace_picked: false,
    });

    // First Enter advances from Harness step to Workspace step: where the
    // harness runs is asked before who holds it.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert_eq!(a.attached_session(), None, "must not attach behind a modal");
    assert_eq!(
        a.session_picker.as_ref().map(|picker| picker.step),
        Some(SessionPickerStep::Workspace),
        "the picker should have advanced to its workspace step"
    );

    // This app has no local harnesses, so nothing completes the empty query.
    // Stand a choice in for the completion pass, which is what the workspace
    // step's Enter reads.
    if let Some(picker) = &mut a.session_picker {
        picker.workspace_choices = vec![WorkspaceChoice {
            path: ".".into(),
            source: "recent",
        }];
        picker.workspace_index = 0;
    }

    // Second Enter takes the workspace and starts the harness — the workspace
    // step is the last one, exactly as its own title says. There is no control
    // question after it: a harness started by hand is the operator's.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(
        a.session_picker.is_none(),
        "the workspace step is the last one, so Enter must close the picker"
    );
    assert_eq!(a.attached_session(), None, "must not attach behind a modal");
}

#[test]
fn enter_on_a_harness_asks_before_taking_it() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    // The render pass records the harness behind the visible pane. This app
    // hosts nothing, so the handover question has nothing to ask about and says
    // so — the point being that Enter is consumed by the harness path rather
    // than returning to the composer or submitting a turn, and that it never
    // attaches on its own.
    a.pane_session = Some("just-exited".to_string());

    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(a.sessions_rail_focused());
    assert!(a.status().contains("not hosting"), "{}", a.status());
    assert!(a.handback_prompt.is_none());
    assert_eq!(a.attached_session(), None);
}

#[test]
fn d_on_a_selected_harness_swaps_the_pane_for_its_diff() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());

    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert_eq!(
        a.tab(),
        "Sessions",
        "the diff replaces the pane, not the tab"
    );
    assert_eq!(a.pane_view, PaneView::Diff);
    assert_eq!(a.rail_session.as_deref(), Some("selected-harness"));
    assert_eq!(a.draft.text, "", "the shortcut must not type into chat");
}

#[test]
fn d_again_puts_the_harness_back_in_the_pane() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());

    a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(a.pane_view, PaneView::Harness);
    assert_eq!(a.draft.text, "", "neither press may reach the composer");
}

#[test]
fn esc_closes_the_pane_diff() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());
    a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(a.pane_view, PaneView::Harness);
}

#[test]
fn esc_cancels_the_open_diff_baseline_picker_before_closing_the_diff() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());
    a.pane_view = PaneView::Diff;
    a.open_change_baseline_picker();

    a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(!a.changes.picking_baseline);
    assert_eq!(a.pane_view, PaneView::Diff);
}

#[test]
fn the_open_diff_owns_its_own_navigation_keys() {
    // `k` kills the harness from the terminal view; over the diff it is the
    // Changes cursor, exactly as on the tab. A view that replaced the pane but
    // left the pane's bindings in place would kill a session on a keypress the
    // operator meant as "move down one line".
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());
    a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    a.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

    assert!(a.harness_close_armed.is_none(), "{}", a.status());
    assert_eq!(a.pane_view, PaneView::Diff);
    assert_eq!(a.draft.text, "");
}

#[test]
fn enter_applies_the_open_diff_baseline_picker() {
    let mut a = app();
    a.tab_index = tab("Sessions");
    a.pane_session = Some("selected-harness".to_owned());
    a.pane_view = PaneView::Diff;
    a.open_change_baseline_picker();

    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(a.changes.picking_baseline);
    assert!(a.handback_prompt.is_none());
    assert_eq!(a.attached_session(), None);
    assert!(
        a.status().contains("No session Git repository"),
        "{}",
        a.status()
    );
}

#[test]
fn clicking_a_context_chunk_selects_it() {
    // Context is a Settings *subpage*, not a top-level tab, so the click router
    // has to match on the subpage — matching on the tab made this branch
    // unreachable and clicking a chunk silently did nothing.
    let mut a = app();
    let _ = a.focus_settings_subpage("Context");
    assert_eq!(a.settings_subpage(), "Context");

    a.contexts = vec![
        medulla::runtime::ContextItem {
            ref_: "a".into(),
            kind: "file".into(),
            bytes: 10,
            content: "alpha".into(),
        },
        medulla::runtime::ContextItem {
            ref_: "b".into(),
            kind: "file".into(),
            bytes: 20,
            content: "bravo".into(),
        },
    ];
    a.hit_context = Some(ratatui::layout::Rect::new(0, 5, 40, 10));

    // Second row inside the hit rect selects the second chunk.
    let _ = a.handle_click(3, 6);
    assert_eq!(a.context_index, 1);

    // A click past the last chunk leaves the selection alone.
    let _ = a.handle_click(3, 9);
    assert_eq!(a.context_index, 1);

    // A click outside the rect is ignored entirely.
    let _ = a.handle_click(3, 40);
    assert_eq!(a.context_index, 1);
}

#[cfg(feature = "workflows")]
#[test]
fn the_wheel_scrolls_the_workflow_preview_only_when_the_pointer_is_over_it() {
    let mut a = app();
    a.tab_index = tab("Workflows");
    a.hit_workflow_preview = Some(ratatui::layout::Rect::new(40, 10, 30, 12));

    a.scroll_at(50, 15, false);
    assert_eq!(a.wf.preview_scroll, 3);

    a.scroll_at(10, 15, false);
    assert_eq!(a.wf.preview_scroll, 3);

    a.scroll_at(50, 15, true);
    assert_eq!(a.wf.preview_scroll, 0);
}

#[test]
fn routing_strategies_render_host_and_subscription_groups() {
    let mut a = app();
    a.tab_index = tab("Hosts");
    a.routing_index = types::RP_STRATEGIES;
    a.routing_focused = true;

    let out = render(&mut a);
    assert!(out.contains("Host strategy"), "{out}");
    assert!(out.contains("CPU First"), "{out}");
    assert!(out.contains("Subscription strategy"), "{out}");
    assert!(out.contains("Most Available Budget"), "{out}");
}

#[test]
fn subscription_strategy_navigation_persists_and_emits_its_own_operation() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app();
    a.set_config_path(dir.path().join("config.toml"));
    a.tab_index = tab("Hosts");
    a.routing_index = types::RP_STRATEGIES;
    a.routing_focused = true;

    a.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(matches!(
        cmd,
        Some(Cmd::WorkerOp(
            medulla::runtime::WorkerOp::ApplySubscriptionStrategy {
                strategy: medulla::runtime::SubscriptionRoutingStrategy::MostAvailableBudget
            }
        ))
    ));
    let saved = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        saved.contains("subscriptionRoutingStrategy = \"mostAvailableBudget\""),
        "{saved}"
    );
}

// --- screen subscriptions follow the selection -----------------------------

/// Put the cursor on the first task sublane in the rail, returning the command
/// that produced.
fn select_first_task(app: &mut App) -> Option<Cmd> {
    app.tab_index = tab("Sessions");
    let rows = app.rail_rows();
    let idx = rows.iter().position(|r| {
        // A dispatched task is a *session* of its agent now, not a sublane of
        // its lane: one row type for everything an agent is running.
        matches!(r, super::rail::RailRow::Session(session) if session.task.is_some())
    })?;
    app.set_rail_cursor(idx);
    app.retarget_watch()
}

#[test]
fn selecting_a_task_asks_to_watch_it() {
    let mut app = app();
    let cmd = select_first_task(&mut app).expect("the fixture has a selectable task");
    let Cmd::WatchTask { stop, start } = cmd else {
        panic!("selecting a task should retarget the watch");
    };
    assert!(stop.is_none(), "nothing was being watched before");
    let (_, task_id) = start.expect("a task to start watching");
    assert!(!task_id.is_empty());
    assert!(app.watching.is_some(), "the target is remembered");
}

#[test]
fn killing_a_watched_harness_requires_confirmation() {
    let mut app = app_with_running_task();
    select_first_task(&mut app).expect("the fixture has a selectable task");

    let armed = app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    assert!(armed.is_none(), "arming must not kill the harness");
    assert!(app.kill_armed.is_some());
    assert!(app.status().contains("y confirm"));

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::KillTask { .. })));
    assert!(app.kill_armed.is_none());
}

#[test]
fn killing_resolves_the_current_rail_selection_instead_of_the_cached_watch() {
    let mut app = app_with_running_task();
    select_first_task(&mut app).expect("the fixture has a selectable task");
    let selected = app.watch_target().expect("the selected task is watchable");
    app.watching = Some(("stale-worker".into(), "stale-task".into()));

    app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    assert_eq!(app.kill_armed.as_ref(), Some(&selected));

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    let Some(Cmd::KillTask { worker, task_id }) = cmd else {
        panic!("confirming should kill the selected task");
    };
    assert_eq!((worker, task_id), selected);
}

#[test]
fn a_paste_cancels_a_harness_kill_instead_of_slipping_past_it() {
    // The confirmation owns exactly one input, and `on_key` enforces that ahead
    // of every other target. A paste is an input too: without this it neither
    // answered nor cancelled, so the payload was routed somewhere while the
    // question stayed armed for whatever key came next.
    let mut app = app_with_running_task();
    select_first_task(&mut app).expect("the fixture has a selectable task");

    app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    assert!(app.kill_armed.is_some(), "the question is up");

    let cmd = app.on_event(crossterm::event::Event::Paste("y".into()));

    assert!(cmd.is_none(), "a pasted `y` is not a confirmation");
    assert!(app.kill_armed.is_none(), "the question consumed the paste");
    assert!(app.status().contains("cancelled"), "{}", app.status());
    // The load-bearing half: the question must be *gone*, so the next key is an
    // ordinary keystroke again rather than an answer to a prompt nobody can
    // still see.
    let after = app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(
        !matches!(after, Some(Cmd::KillTask { .. })),
        "a paste must not leave the kill armed for the next keypress"
    );
    assert!(
        app.kill_armed.is_none(),
        "and the prompt is gone rather than waiting for another key"
    );
}

#[test]
fn any_other_key_cancels_a_harness_kill() {
    let mut app = app_with_running_task();
    select_first_task(&mut app).expect("the fixture has a selectable task");

    app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert!(cmd.is_none());
    assert!(app.kill_armed.is_none());
    assert!(app.status().contains("cancelled"));
}

#[test]
fn reselecting_the_same_task_does_not_resubscribe() {
    // Every subscribe carries `resync: true`, so re-issuing one on each
    // keystroke would make the worker resend a full frame each time — the
    // expensive thing this protocol exists to avoid.
    let mut app = app();
    select_first_task(&mut app).expect("a task to select");
    assert!(
        app.retarget_watch().is_none(),
        "an unchanged selection must not retarget"
    );
}

#[test]
fn leaving_the_agents_tab_releases_the_subscription() {
    // A stream nobody is looking at still costs the worker a sample, a ratchet
    // advance and a send every tick.
    let mut app = app();
    select_first_task(&mut app).expect("a task to select");
    app.tab_index = tab("Overview");

    let Some(Cmd::WatchTask { stop, start }) = app.retarget_watch() else {
        panic!("leaving the tab should release the watch");
    };
    assert!(stop.is_some(), "the previous stream is stopped");
    assert!(start.is_none(), "and nothing replaces it");
    assert!(app.watching.is_none());
}

#[test]
fn selecting_an_action_row_rather_than_a_task_watches_nothing() {
    // Streams are addressed by task. The action row names none, and guessing one
    // would watch work the operator did not point at.
    let mut app = app();
    app.set_local_sessions(super::rail::tests::shell_harnesses(
        crate::worker::pty::PtyManager::new(),
    ));
    app.tab_index = tab("Sessions");
    let rows = app.rail_rows();
    let idx = rows
        .iter()
        .position(|r| matches!(r, super::rail::RailRow::NewSession))
        .expect("a hosting fixture offers the action row");
    app.set_rail_cursor(idx);
    assert!(app.retarget_watch().is_none());
    assert!(app.watching.is_none());
}
