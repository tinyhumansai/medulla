//! Focused unit tests for the [`App`] screen: that every tab renders, the async
//! header toggle shows, and the composer/slash-command dispatch behaves.

use super::*;
use std::sync::Arc;

use super::types::RP_HARNESSES;
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
        l.config.tinyplace = Some(medulla::config::TinyplaceConfig::default());
        l
    };
    App::new(rt, loaded)
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
fn slash_help_switches_tab() {
    let mut a = app();
    a.tab_index = 1;
    let _ = a.execute("/help".into());
    assert_eq!(a.tab(), "Settings");
    assert_eq!(a.settings_subpage(), "Help");
}

#[test]
fn unknown_command_sets_status() {
    let mut a = app();
    let _ = a.execute("/bogus".into());
    assert!(a.status.contains("Unknown command"));
}

#[test]
fn plain_text_returns_submit_cmd() {
    let mut a = app();
    a.tab_index = 1;
    let cmd = a.execute("hello world".into());
    assert!(matches!(cmd, Some(Cmd::Submit(s)) if s == "hello world"));
    assert_eq!(a.status, "Cycle running…");
}

#[test]
fn typing_inserts_into_draft() {
    let mut a = app();
    a.tab_index = 1;
    for ch in "hi".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(a.draft.text, "hi");
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(a.draft.text, "hi\n");
}

#[test]
fn enter_on_a_harness_uses_the_attach_path_in_normal_mode() {
    let mut a = app();
    a.tab_index = tab("Agents");
    a.focus_agents_rail();
    // The render pass records the harness behind the visible pane. A vanished
    // session exercises the refusal path without opening a real child here;
    // importantly, Enter is still consumed as an attach attempt instead of
    // returning to the composer or submitting a turn.
    a.harness_pane_session = Some("just-exited".to_string());

    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(a.agents_rail_focused());
    assert!(a.status().contains("harness has exited"), "{}", a.status());
    assert_eq!(a.attached_harness(), None);
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
    app.tab_index = tab("Agents");
    let rows = app.rail_rows();
    let idx = rows.iter().position(|r| {
        matches!(
            r,
            super::rail::RailRow::Agent(crate::ui::agents::AgentRow::Sub { .. })
        )
    })?;
    app.agent_index = idx;
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
fn selecting_a_lane_rather_than_a_task_watches_nothing() {
    // Streams are addressed by task. A lane names none, and guessing one of its
    // tasks would watch work the operator did not point at.
    let mut app = app();
    app.tab_index = tab("Agents");
    let rows = app.rail_rows();
    let idx = rows
        .iter()
        .position(|r| {
            matches!(
                r,
                super::rail::RailRow::Agent(crate::ui::agents::AgentRow::Lane { .. })
            )
        })
        .expect("the fixture has a lane row");
    app.agent_index = idx;
    assert!(app.retarget_watch().is_none());
    assert!(app.watching.is_none());
}
