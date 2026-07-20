//! Feature tests for two paths that had no coverage: copying the chat to the
//! real clipboard (as opposed to the injected capture sink used elsewhere), and
//! the feedback submission prompt's empty-input cancels.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::events::TuiEvent;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn type_str(app: &mut App, s: &str) {
    for ch in s.chars() {
        app.on_event(key(KeyCode::Char(ch)));
    }
}

fn chat_app() -> App {
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(
        rt.clone(),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    rt.script_event(TuiEvent::User {
        body: "what is the plan".into(),
    });
    rt.script_event(TuiEvent::Assistant {
        body: "branch, then commit".into(),
    });
    app.refresh_snapshot();
    app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap();
    app
}

#[test]
fn copying_a_chat_reports_what_went_where() {
    // Without a capture sink the copy goes to the real clipboard (or falls back
    // to an OSC 52 escape). Either way the user must be told which happened —
    // OSC 52 is silent from the app's side, so "copied" alone would be a lie.
    let mut app = chat_app();
    app.on_event(ctrl('y'));
    let status = app.status().to_string();
    assert!(
        status.contains("Copied chat") || status.contains("Sent chat"),
        "names the scope and the mechanism: {status}"
    );
    assert!(status.contains("line"), "reports the size: {status}");
}

#[test]
fn copying_an_empty_chat_says_there_is_nothing_to_copy() {
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap();
    app.on_event(ctrl('y'));
    assert!(app.status().contains("Nothing to copy"), "{}", app.status());
}

#[test]
fn an_empty_feedback_title_cancels_the_submission() {
    // The prompt is two steps; submitting nothing at either step must abandon
    // the whole thing rather than post a blank item.
    let mut app = chat_app();
    let _ = app.focus_settings_subpage("Feedback");
    app.on_event(key(KeyCode::Char('n'))); // new feature request
    let cmd = app.on_event(key(KeyCode::Enter)); // …with no title
    assert!(cmd.is_none(), "nothing is submitted");
    assert!(
        app.status().contains("cancelled"),
        "says it was cancelled: {}",
        app.status()
    );
}

#[test]
fn an_empty_feedback_description_cancels_the_submission() {
    let mut app = chat_app();
    let _ = app.focus_settings_subpage("Feedback");
    app.on_event(key(KeyCode::Char('b'))); // new bug report
    type_str(&mut app, "a real title");
    app.on_event(key(KeyCode::Enter)); // advances to the description step
    let cmd = app.on_event(key(KeyCode::Enter)); // …with no description
    assert!(cmd.is_none(), "nothing is submitted");
    assert!(
        app.status().contains("cancelled"),
        "says it was cancelled: {}",
        app.status()
    );
}

#[test]
fn answering_with_no_task_selected_explains_itself() {
    let mut app = chat_app();
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();
    app.on_event(key(KeyCode::Char('A')));
    assert!(
        app.status().contains("Select a task"),
        "points at what to do first: {}",
        app.status()
    );
}
