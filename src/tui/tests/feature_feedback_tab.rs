//! Feature-level tests for the public feedback board ("Feedback" tab): browsing
//! and selection, the sort/filter cycles, voting (including retraction), the
//! comment pane's three states, and the type/status labels.
//!
//! These drive the `App` through real key events with a `MockRuntime`, mirroring
//! `feature_memory_tab.rs`. The board's data arrives via the `set_*` seams the
//! command loop uses, so no network is involved.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use medulla::client::{FeedbackComment, FeedbackItem, FeedbackPage, FeedbackStatus, FeedbackType};
use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

fn feedback_tab() -> usize {
    TABS.iter()
        .position(|t| *t == "Feedback")
        .expect("Feedback tab")
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// Build an item through deserialization — the type is wire-shaped and has no
/// constructor, so this also exercises its serde contract.
fn item(id: &str, kind: &str, title: &str, my_vote: i8) -> FeedbackItem {
    serde_json::from_value(json!({
        "id": id,
        "type": kind,
        "title": title,
        "body": format!("body of {title}"),
        "status": "open",
        "createdByName": "ada",
        "upvoteCount": 3,
        "downvoteCount": 1,
        "score": 2,
        "commentCount": 1,
        "myVote": my_vote,
        "createdAt": "2026-01-05T10:00:00Z",
    }))
    .expect("item fixture")
}

fn comment(body: &str, who: Option<&str>) -> FeedbackComment {
    serde_json::from_value(json!({
        "id": "c1",
        "userName": who,
        "body": body,
        "createdAt": "2026-01-05T11:00:00Z",
    }))
    .expect("comment fixture")
}

/// An app on the Feedback tab with a loaded page of two items.
fn board() -> App {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = feedback_tab();
    app.set_feedback_page(Some(FeedbackPage {
        items: vec![
            item("f1", "feature", "Dark mode", 0),
            item("f2", "bug", "Crash on resume", 1),
        ],
        total: 2,
    }));
    app
}

#[test]
fn the_board_lists_items_with_their_type_and_score() {
    let mut app = board();
    let out = render(&mut app, 120, 32);

    assert!(out.contains("Dark mode"), "first item: {out}");
    assert!(out.contains("Crash on resume"), "second item: {out}");
    assert!(out.contains("feat"), "feature label: {out}");
    assert!(out.contains("bug"), "bug label: {out}");
}

#[test]
fn selecting_a_row_requests_its_comments_once() {
    let mut app = board();

    // Selecting the first row asks for its detail.
    match app.feedback_detail_cmd() {
        Some(Cmd::LoadFeedbackDetail(id)) => assert_eq!(id, "f1"),
        other => panic!("expected a detail load, got {other:?}"),
    }

    // Once those comments are in, the same row does not re-request them.
    app.set_feedback_comments("f1".into(), vec![comment("nice idea", Some("bob"))]);
    assert!(
        app.feedback_detail_cmd().is_none(),
        "comments already loaded for the selected row"
    );
}

#[test]
fn navigation_moves_the_selection_and_clamps_at_both_ends() {
    let mut app = board();
    assert_eq!(app.feedback_index(), 0);

    // Down past the end clamps to the last row.
    let cmd = app.on_event(key(KeyCode::Down));
    assert_eq!(app.feedback_index(), 1);
    assert!(
        matches!(cmd, Some(Cmd::LoadFeedbackDetail(ref id)) if id == "f2"),
        "moving selection loads the new row's comments: {cmd:?}"
    );
    app.on_event(key(KeyCode::Down));
    assert_eq!(app.feedback_index(), 1, "clamped at the last row");

    // Up past the start clamps to the first.
    app.on_event(key(KeyCode::Up));
    assert_eq!(app.feedback_index(), 0);
    app.on_event(key(KeyCode::Up));
    assert_eq!(app.feedback_index(), 0, "clamped at the first row");

    // j/k mirror the arrows.
    app.on_event(key(KeyCode::Char('j')));
    assert_eq!(app.feedback_index(), 1);
    app.on_event(key(KeyCode::Char('k')));
    assert_eq!(app.feedback_index(), 0);
}

#[test]
fn the_filter_cycles_through_everything_features_and_bugs() {
    let mut app = board();
    assert!(app.feedback_query().kind.is_none(), "starts unfiltered");

    app.on_event(key(KeyCode::Char('f')));
    assert_eq!(app.feedback_query().kind, Some(FeedbackType::Feature));

    app.on_event(key(KeyCode::Char('f')));
    assert_eq!(app.feedback_query().kind, Some(FeedbackType::Bug));

    app.on_event(key(KeyCode::Char('f')));
    assert!(app.feedback_query().kind.is_none(), "cycles back to all");
}

#[test]
fn the_filter_resets_paging_and_reloads() {
    let mut app = board();

    let cmd = app.on_event(key(KeyCode::Char('f')));
    let query = app.feedback_query();

    assert_eq!(query.page, 1, "a new filter starts at the first page");
    assert!(
        matches!(cmd, Some(Cmd::LoadFeedback(_))),
        "filtering reloads the board: {cmd:?}"
    );
}

#[test]
fn the_sort_cycle_reloads_the_board() {
    let mut app = board();
    let first = app.feedback_sort();

    let cmd = app.on_event(key(KeyCode::Char('s')));

    assert_ne!(app.feedback_sort(), first, "sort advanced");
    assert!(
        matches!(cmd, Some(Cmd::LoadFeedback(_))),
        "sorting reloads the board: {cmd:?}"
    );
}

#[test]
fn voting_emits_the_vote_and_retracts_when_repeated() {
    let mut app = board();

    // First row has no vote yet, so u casts an upvote.
    match app.on_event(key(KeyCode::Char('u'))) {
        Some(Cmd::VoteFeedback { id, value }) => {
            assert_eq!(id, "f1");
            assert_eq!(value, 1);
        }
        other => panic!("expected a vote, got {other:?}"),
    }

    // Second row is already upvoted, so u retracts rather than double-voting.
    app.on_event(key(KeyCode::Down));
    match app.on_event(key(KeyCode::Char('u'))) {
        Some(Cmd::VoteFeedback { id, value }) => {
            assert_eq!(id, "f2");
            assert_eq!(value, 0, "repeating your own vote retracts it");
        }
        other => panic!("expected a retraction, got {other:?}"),
    }

    // A downvote on an upvoted row replaces it outright.
    match app.on_event(key(KeyCode::Char('d'))) {
        Some(Cmd::VoteFeedback { value, .. }) => assert_eq!(value, -1),
        other => panic!("expected a downvote, got {other:?}"),
    }
}

#[test]
fn an_updated_item_replaces_its_row_in_place() {
    let mut app = board();

    app.apply_feedback_item(item("f2", "bug", "Crash on resume", -1));

    let updated = app
        .feedback_items()
        .iter()
        .find(|i| i.id == "f2")
        .expect("row still present");
    assert_eq!(updated.my_vote, -1, "the tallies were refreshed in place");
    assert_eq!(app.feedback_items().len(), 2, "no row was added or lost");
}

#[test]
fn the_comment_pane_distinguishes_loading_empty_and_present() {
    let mut app = board();

    // Nothing fetched yet for the selected row.
    let out = render(&mut app, 120, 32);
    assert!(out.contains("Loading comments"), "loading state: {out}");

    // Fetched, but the item has none.
    app.set_feedback_comments("f1".into(), Vec::new());
    let out = render(&mut app, 120, 32);
    assert!(out.contains("No comments yet"), "empty state: {out}");

    // Fetched with content — the author and body both render.
    app.set_feedback_comments(
        "f1".into(),
        vec![
            comment("first line\nsecond line", Some("bob")),
            comment("anonymous note", None),
        ],
    );
    let out = render(&mut app, 120, 40);
    assert!(out.contains("comment(s)"), "comment header: {out}");
    assert!(out.contains("bob"), "named author: {out}");
    assert!(out.contains("second line"), "multi-line body: {out}");
    assert!(
        out.contains("someone"),
        "an author-less comment falls back to a placeholder: {out}"
    );
}

#[test]
fn an_empty_board_still_renders() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = feedback_tab();
    app.set_feedback_page(Some(FeedbackPage {
        items: Vec::new(),
        total: 0,
    }));

    let out = render(&mut app, 120, 32);
    assert!(!out.trim().is_empty(), "the tab renders with no items");
    assert!(app.feedback_detail_cmd().is_none(), "nothing to select");
}

#[test]
fn commenting_without_a_selection_explains_itself() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = feedback_tab();
    app.set_feedback_page(Some(FeedbackPage {
        items: Vec::new(),
        total: 0,
    }));

    app.on_event(key(KeyCode::Char('c')));

    let out = render(&mut app, 120, 32);
    assert!(
        out.contains("Select an item"),
        "the status should say why nothing happened: {out}"
    );
}

#[test]
fn type_and_status_labels_cover_every_variant() {
    // These drive the list rows, including the catch-all variants the backend
    // may send for values this client does not model yet.
    assert_eq!(FeedbackType::Feature.label(), "feat");
    assert_eq!(FeedbackType::Bug.label(), "bug");
    assert_eq!(FeedbackType::Other.label(), "misc");

    // The wire value sent on submit; an unmodelled variant falls back to
    // "feature" so a round-tripped item is still submittable.
    assert_eq!(FeedbackType::Feature.as_str(), "feature");
    assert_eq!(FeedbackType::Bug.as_str(), "bug");
    assert_eq!(FeedbackType::Other.as_str(), "feature");

    assert_eq!(FeedbackStatus::Open.label(), "open");
    assert_eq!(FeedbackStatus::Planned.label(), "planned");
    assert_eq!(FeedbackStatus::Completed.label(), "done");
    assert_eq!(FeedbackStatus::Other.label(), "?");
}

#[test]
fn the_header_names_the_active_filter_and_sort() {
    let mut app = board();

    // All three sort labels appear as the cycle advances.
    let mut seen = vec![app.feedback_sort()];
    for _ in 0..2 {
        app.on_event(key(KeyCode::Char('s')));
        seen.push(app.feedback_sort());
    }
    seen.sort_unstable();
    assert_eq!(
        seen,
        vec!["hot", "new", "top"],
        "the cycle covers every ordering"
    );

    // Filtering to bugs is reflected in the header.
    app.set_feedback_page(Some(FeedbackPage {
        items: vec![item("f2", "bug", "Crash on resume", 0)],
        total: 1,
    }));
    app.on_event(key(KeyCode::Char('f'))); // features
    app.on_event(key(KeyCode::Char('f'))); // bugs
    app.set_feedback_page(Some(FeedbackPage {
        items: vec![item("f2", "bug", "Crash on resume", 0)],
        total: 1,
    }));
    let out = render(&mut app, 120, 32);
    assert!(out.contains("bugs"), "the bug filter is named: {out}");
}

#[test]
fn an_empty_board_mid_load_says_it_is_loading() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = feedback_tab();

    // Cycling the sort marks the board loading and clears nothing, so an empty
    // board renders the loading hint rather than the "nothing here" prompt.
    app.on_event(key(KeyCode::Char('s')));

    let out = render(&mut app, 120, 32);
    assert!(out.contains("Loading"), "loading hint: {out}");
    assert!(
        !out.contains("Nothing here yet"),
        "a loading board is not an empty one: {out}"
    );
}

#[test]
fn a_settled_empty_board_invites_the_first_submission() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = feedback_tab();
    app.set_feedback_page(Some(FeedbackPage {
        items: Vec::new(),
        total: 0,
    }));

    let out = render(&mut app, 120, 32);
    assert!(out.contains("Nothing here yet"), "empty prompt: {out}");
    assert!(out.contains("item(s)"), "settled count: {out}");
}
