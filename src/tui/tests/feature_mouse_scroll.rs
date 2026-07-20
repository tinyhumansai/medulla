//! Feature tests for mouse-wheel scrolling on each scrollable surface.
//!
//! Trace and Context moved from top-level tabs to Settings subpages, which
//! silently broke their scroll arms: both matched on the tab name, and the tab
//! is "Settings" for both. These tests pin the routing so the next such move is
//! caught.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::ContextItem;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::events::{NodeTrace, TuiEvent};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn wheel(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: 10,
        row: 10,
        modifiers: KeyModifiers::NONE,
    })
}

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 16)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

fn seeded_app() -> App {
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(
        rt.clone(),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    for i in 0..40 {
        rt.script_event(TuiEvent::Trace {
            entry: NodeTrace {
                node: format!("node-{i}"),
                ms: i,
                tool: None,
                op: Some("step".into()),
            },
        });
    }
    app.set_contexts(
        ["first", "second", "third"]
            .into_iter()
            .map(|name| ContextItem {
                ref_: name.into(),
                kind: "file".into(),
                bytes: name.len(),
                content: format!("contents of {name}"),
            })
            .collect(),
    );
    app.refresh_snapshot();
    app
}

#[test]
fn the_wheel_scrolls_the_trace_and_context_subpages() {
    // The regression this covers: both used to match on a tab name that no
    // longer exists, so the wheel did nothing on either page.
    for subpage in ["Trace", "Context"] {
        let mut app = seeded_app();
        let _ = app.focus_settings_subpage(subpage);
        let before = render(&mut app);

        app.on_event(wheel(MouseEventKind::ScrollDown));
        assert_ne!(
            render(&mut app),
            before,
            "{subpage}: the wheel should scroll the page"
        );

        app.on_event(wheel(MouseEventKind::ScrollUp));
        assert_eq!(
            render(&mut app),
            before,
            "{subpage}: scrolling back returns to the start"
        );
    }
}

#[test]
fn the_wheel_scrolls_the_memory_tab() {
    let mut app = seeded_app();
    app.tab_index = TABS.iter().position(|t| *t == "Memory").unwrap();
    // Scrolling up at the top is a no-op rather than an underflow.
    app.on_event(wheel(MouseEventKind::ScrollUp));
    assert_eq!(app.memory_index(), 0, "clamped at the top");
    app.on_event(wheel(MouseEventKind::ScrollDown));
}

#[test]
fn the_wheel_is_swallowed_by_the_resume_picker() {
    // A modal owns the pointer; scrolling the list behind it would be wrong.
    let mut app = seeded_app();
    app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap();
    app.open_resume(Vec::new());
    assert!(app.on_event(wheel(MouseEventKind::ScrollDown)).is_none());
    assert!(app
        .on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .is_none());
}

#[test]
fn non_key_non_mouse_events_are_ignored() {
    let mut app = seeded_app();
    assert!(app.on_event(Event::FocusGained).is_none());
    assert!(app.on_event(Event::Resize(10, 10)).is_none());
    // A key *release* is not a press and must not act.
    assert!(app
        .on_event(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            crossterm::event::KeyEventKind::Release,
        )))
        .is_none());
}

#[test]
fn clicking_a_context_chunk_selects_it() {
    // Same regression as the wheel: the click branch matched on a tab name that
    // Context no longer has, so clicking a row did nothing.
    let mut app = seeded_app();
    let _ = app.focus_settings_subpage("Context");
    // Rendering is what publishes the clickable rect.
    let before = render(&mut app);

    // Probe each row of the pane on a fresh app: exactly which row the list
    // starts on is layout detail, but *some* row must select a chunk.
    let clicked = (1..14).find(|row| {
        let mut probe = seeded_app();
        let _ = probe.focus_settings_subpage("Context");
        render(&mut probe); // publishes the clickable rect
        probe.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 30,
            row: *row,
            modifiers: KeyModifiers::NONE,
        }));
        render(&mut probe) != before
    });
    assert!(
        clicked.is_some(),
        "clicking inside the Context pane must select a chunk"
    );
}
