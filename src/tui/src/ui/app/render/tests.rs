//! Unit tests for the outer render chrome: the compact tab labels, the picker's
//! scroll window, and the per-tab shortcut line and keyboard hand-back.
//!
//! The lane presence-glyph and chat-transcript tests that used to live here went
//! with the surfaces they covered — the rail lists sessions rather than lanes,
//! and the orchestrator's transcript is no longer drawn in the TUI at all.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use crate::ui::app::changes::types::ChangedFile;
use crate::ui::app::types::{tab_pos, PaneView};
use crate::ui::app::App;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

#[test]
fn compact_tab_labels_shorten_the_current_wide_destinations() {
    assert_eq!(super::compact_tab_label("Sessions", true), "Sess");
    assert_eq!(super::compact_tab_label("Workflows", true), "Flows");
    assert_eq!(super::compact_tab_label("Subconscious", true), "Sub");
    assert_eq!(super::compact_tab_label("Feedback", true), "Feed");
    assert_eq!(super::compact_tab_label("Settings", true), "Set");
    assert_eq!(super::compact_tab_label("Hosts", true), "Hosts");
}

#[test]
fn harness_choice_window_keeps_the_selection_visible() {
    assert_eq!(
        super::session_modals::harness_choice_window(20, 0, 13),
        0..13
    );
    assert_eq!(
        super::session_modals::harness_choice_window(20, 10, 13),
        4..17
    );
    assert_eq!(
        super::session_modals::harness_choice_window(20, 19, 13),
        7..20
    );
    assert_eq!(super::session_modals::harness_choice_window(2, 1, 13), 0..2);
}

#[test]
fn harness_picker_hint_only_advertises_bound_keys() {
    use crate::ui::app::types::SessionPickerStep;

    let harness = super::session_modals::harness_picker_hint(SessionPickerStep::Harness);
    assert!(
        !harness.contains("Tab complete"),
        "Tab is not bound on the harness step, so the hint must not advertise it: {harness}"
    );
    assert!(
        !harness.contains("Shift+F"),
        "Shift+F is not bound on the harness step, so the hint must not advertise it: {harness}"
    );
    assert!(
        harness.contains("unmanaged"),
        "the harness step still states that a hand-started session is unmanaged"
    );

    let workspace = super::session_modals::harness_picker_hint(SessionPickerStep::Workspace);
    assert!(
        workspace.contains("Tab complete"),
        "the workspace step advertises the completion key it actually binds"
    );
    assert!(
        workspace.contains("Shift+F save favorite"),
        "the workspace step advertises the favorite key it actually binds"
    );
}

#[test]
fn workflow_delete_prompt_renders_the_armed_workflow() {
    let mut app = app();
    app.workflow_delete_armed = Some(("nightly".into(), "Nightly sweep".into()));
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).expect("terminal");

    terminal
        .draw(|f| app.draw_workflow_delete_prompt(f, f.area()))
        .expect("draw");
    let output: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(output.contains("Delete workflow"), "{output}");
    assert!(
        output.contains("Delete \"Nightly sweep\" permanently?"),
        "{output}"
    );
    assert!(output.contains("[Delete] remove workflow"), "{output}");
}

#[test]
fn workflow_delete_prompt_draws_nothing_when_unarmed() {
    let mut app = app();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).expect("terminal");

    terminal
        .draw(|f| app.draw_workflow_delete_prompt(f, f.area()))
        .expect("draw");

    assert!(terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .all(|cell| cell.symbol() == " "));
}
#[test]
fn leaving_the_agents_tab_takes_the_keyboard_back_from_an_attached_harness() {
    // The bug this pins: `release_session` was only reached from
    // `sessions_selection`, which runs only while the Sessions tab is being drawn.
    // It notices the *cursor* moving off the attached session and has nothing to
    // say once the operator has left the tab altogether — so focus stayed
    // `Attached`, and every keystroke meant for the tab now on screen was typed
    // into a harness pane the operator could no longer see, with `Ctrl-]` the
    // only way out of a mode with no visible cause.
    //
    // Only the leaving case is asserted here. "Stays attached while the pane is
    // on screen" needs a live pty session for the selection to resolve to, which
    // is covered against a real child in `ui::harness_pane`; this fixture has no
    // harnesses, so the Sessions tab would release focus for the ordinary reason
    // (nothing under the cursor) and prove nothing about the tab switch.
    let mut app = app();
    app.harness_focus = crate::ui::harness_pane::HarnessFocus::Attached("w_1".to_string());
    app.tab_index = crate::ui::app::types::TABS
        .iter()
        .position(|t| *t != "Sessions")
        .expect("some other tab");

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");

    assert_eq!(
        app.attached_session(),
        None,
        "keys must not reach a harness the operator has navigated away from"
    );
}

#[test]
fn a_stale_harness_diff_does_not_advertise_agents_shortcuts_on_another_tab() {
    let mut app = app();
    app.tab_index = tab_pos("Overview");
    app.pane_view = PaneView::Diff;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draw");
    let output: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(output.contains("d session diff"), "{output}");
    assert!(!output.contains("d/Esc harness"), "{output}");
}

/// Put the harness diff pane in front of a patch whose selected line is far
/// wider than the diff pane, so one rendered row wraps past the whole viewport.
///
/// The diff is reached only as a session pane (`d` on a row) now that the
/// top-level Changes tab is gone, so this renders through `draw_harness_diff`
/// — the same path the Sessions tab takes — rather than a removed tab arm.
fn app_on_an_oversized_diff_line() -> App {
    let mut app = app();
    app.tab_index = tab_pos("Sessions");
    app.pane_view = PaneView::Diff;
    app.changes.root = Some(std::path::PathBuf::from("/repo"));
    app.changes.baseline = Some("baseline".to_owned());
    app.changes.files = vec![ChangedFile {
        status: "M".into(),
        path: std::path::PathBuf::from("wide.txt"),
        origins: Vec::new(),
    }];
    app.changes.patch = vec![
        "@@ -1,1 +1,1 @@".to_owned(),
        format!("+{}", "word ".repeat(800)),
        " tail".to_owned(),
    ];
    app.changes.cursor = 1;
    app
}

#[test]
fn a_wrapped_diff_bounds_the_scroll_by_the_rows_it_actually_occupies() {
    let mut app = app_on_an_oversized_diff_line();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");

    terminal
        .draw(|f| app.draw_harness_diff(f, f.area()))
        .expect("draw");

    // The oversized line wraps to many rows, so the bound has to exceed the
    // three logical patch lines rather than counting them one row each.
    assert!(
        app.changes.max_scroll > 3,
        "wrapped rows must widen the scroll bound: {}",
        app.changes.max_scroll
    );
}

#[test]
fn a_cursor_on_an_oversized_line_holds_a_stable_scroll_offset() {
    let mut app = app_on_an_oversized_diff_line();
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");

    terminal
        .draw(|f| app.draw_harness_diff(f, f.area()))
        .expect("draw");
    let first = app.changes.scroll;
    terminal
        .draw(|f| app.draw_harness_diff(f, f.area()))
        .expect("draw again");

    // Showing the top of the row keeps consecutive frames identical instead of
    // oscillating between the row's top and bottom edges.
    assert_eq!(first, app.changes.scroll);
    assert_eq!(app.changes.scroll, 1, "the header row precedes the cursor");
}
