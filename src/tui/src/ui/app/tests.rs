//! Focused unit tests for the [`App`] screen: that every tab renders, the async
//! header toggle shows, and the composer/slash-command dispatch behaves.

use super::types::SP_FEEDBACK;
use super::*;
use std::sync::Arc;

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

// --- Feedback subpage (Settings > GENERAL > Feedback) ------------------------

/// An app parked on the Feedback subpage with the mock board already loaded.
fn feedback_app() -> App {
    let mut a = app();
    // Enter the content pane, as a user arriving via `/feedback` or Enter does:
    // the board's letter bindings only act on a focused page.
    a.enter_settings_subpage(SP_FEEDBACK);
    let page = futures::executor::block_on(a.runtime.list_feedback(a.feedback_query())).unwrap();
    a.set_feedback_page(page);
    a
}

#[test]
fn slash_feedback_opens_the_board() {
    let mut a = app();
    let cmd = a.execute("/feedback".into());
    assert_eq!(a.tab(), "Settings");
    assert_eq!(a.settings_subpage(), "Feedback");
    assert!(matches!(cmd, Some(Cmd::LoadFeedback(_))));
}

#[test]
fn feedback_tab_renders_rows_and_controls() {
    let mut a = feedback_app();
    let out = render(&mut a);
    assert!(out.contains("Split the Trace tab"), "{out}");
    assert!(out.contains("u upvote"), "{out}");
    assert!(out.contains("sort hot"), "{out}");
}

#[test]
fn feedback_tab_without_a_board_shows_a_sign_in_hint() {
    let mut a = app();
    a.set_settings_subpage(SP_FEEDBACK);
    a.set_feedback_page(None);
    let out = render(&mut a);
    assert!(out.contains("signed-in backend connection"), "{out}");
}

#[test]
fn jk_keys_move_the_selection_and_load_comments() {
    let mut a = feedback_app();
    assert_eq!(a.feedback_index(), 0);
    // As a Settings subpage, Feedback browses with j/k — ↑↓ move the nav.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(a.feedback_index(), 1);
    // Selecting a row whose comments are not loaded asks for them.
    assert!(matches!(cmd, Some(Cmd::LoadFeedbackDetail(id)) if id == "fb-2"));
}

#[test]
fn u_and_d_vote_and_toggle_off_when_repeated() {
    let mut a = feedback_app();
    // fb-1 leads the board and this user has already upvoted it, so `u` retracts.
    assert_eq!(a.feedback_items()[0].my_vote, 1);
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::VoteFeedback { value: 0, .. })));

    // `d` on the same row is a fresh downvote, not a toggle.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::VoteFeedback { value: -1, .. })));
}

#[test]
fn s_cycles_sort_and_f_cycles_the_type_filter() {
    let mut a = feedback_app();
    assert_eq!(a.feedback_sort(), "hot");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "top");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "new");
    a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(a.feedback_sort(), "hot");

    // The filter cycles all → features → bugs → all, reloading each time.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::LoadFeedback(_))));
}

#[test]
fn c_opens_a_comment_prompt_that_submits_the_typed_text() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    for ch in "me too".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match cmd {
        Some(Cmd::CommentFeedback { id, body }) => {
            assert_eq!(id, "fb-1");
            assert_eq!(body, "me too");
        }
        other => panic!("expected CommentFeedback, got {other:?}"),
    }
}

#[test]
fn an_empty_comment_is_cancelled_rather_than_posted() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(cmd.is_none());
}

#[test]
fn n_walks_the_two_step_submit_prompt() {
    let mut a = feedback_app();
    // Step one: the title.
    a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    for ch in "Add X".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    // Submitting the title must not send anything yet — it opens the body step.
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(cmd.is_none());

    // Step two: the body, which submits.
    for ch in "please".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match cmd {
        Some(Cmd::SubmitFeedback { kind, title, body }) => {
            assert_eq!(kind, medulla::client::FeedbackType::Feature);
            assert_eq!(title, "Add X");
            assert_eq!(body, "please");
        }
        other => panic!("expected SubmitFeedback, got {other:?}"),
    }
}

#[test]
fn b_submits_as_a_bug_report() {
    let mut a = feedback_app();
    a.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    for ch in "Crash".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for ch in "boom".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let cmd = a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        cmd,
        Some(Cmd::SubmitFeedback { kind, .. }) if kind == medulla::client::FeedbackType::Bug
    ));
}

#[test]
fn a_vote_result_updates_the_row_in_place() {
    let mut a = feedback_app();
    let mut updated = a.feedback_items()[0].clone();
    updated.score = 99;
    updated.my_vote = 0;
    a.apply_feedback_item(updated);
    assert_eq!(a.feedback_items()[0].score, 99);
    // Applying an update must not move the cursor.
    assert_eq!(a.feedback_index(), 0);
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
    a.tab_index = tab("Routing");
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
    a.tab_index = tab("Routing");
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
fn killing_a_watched_harness_requires_confirmation() {
    let mut app = app();
    select_first_task(&mut app).expect("the fixture has a selectable task");
    app.focus_agents_rail();

    let armed = app.on_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    assert!(armed.is_none(), "arming must not kill the harness");
    assert!(app.kill_armed.is_some());
    assert!(app.status().contains("y confirm"));

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Cmd::KillTask { .. })));
    assert!(app.kill_armed.is_none());
}

#[test]
fn any_other_key_cancels_a_harness_kill() {
    let mut app = app();
    select_first_task(&mut app).expect("the fixture has a selectable task");
    app.focus_agents_rail();
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
