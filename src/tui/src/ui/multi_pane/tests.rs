//! Focused tests for the reusable multi-pane navigation state machine and the
//! sidebar renderer both nav flavours share.

use crossterm::event::KeyCode;
use ratatui::backend::TestBackend;
use ratatui::widgets::Block;
use ratatui::Terminal;

use crate::ui::theme::Theme;

use super::{draw_rows, navigate, NavAction, NavHits, NavRow};

#[test]
fn menu_arrows_move_selection_without_entering_content() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Down, 4, &mut selected, &mut focused, true),
        NavAction::SelectionChanged
    );
    assert_eq!(selected, 1);
    assert!(!focused);
}

#[test]
fn digits_jump_directly_into_a_page() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Char('4'), 4, &mut selected, &mut focused, true),
        NavAction::SelectionChanged
    );
    assert_eq!(selected, 3);
    assert!(focused);
}

#[test]
fn content_escape_respects_a_page_owned_confirmation() {
    let mut selected = 2;
    let mut focused = true;
    assert_eq!(
        navigate(KeyCode::Esc, 4, &mut selected, &mut focused, false),
        NavAction::Unhandled
    );
    assert!(focused);
    assert_eq!(
        navigate(KeyCode::Esc, 4, &mut selected, &mut focused, true),
        NavAction::Left
    );
    assert!(!focused);
}

#[test]
fn structural_keys_are_left_for_the_top_level_app() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Tab, 4, &mut selected, &mut focused, true),
        NavAction::Unhandled
    );
}

/// Draw `rows` into a `width`×`height` terminal, returning its lines and hits.
fn render(
    rows: &[NavRow<'_>],
    nav_focused: bool,
    width: u16,
    height: u16,
) -> (Vec<String>, NavHits) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut hits = NavHits::default();
    terminal
        .draw(|f| {
            let area = f.area();
            hits = draw_rows(
                f,
                area,
                Block::default(),
                &Theme::default(),
                rows,
                nav_focused,
                "hint",
                0,
            );
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let lines = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    (lines, hits)
}

#[test]
fn the_cursor_marker_follows_the_keyboard() {
    // The two sidebars in the app had disagreed about this: one marked the row
    // when the *content* pane held the keyboard, the other when the menu did.
    let rows = [
        NavRow {
            selected: true,
            ..NavRow::new("first")
        },
        NavRow::new("second"),
    ];

    let (focused, _) = render(&rows, true, 20, 6);
    assert!(focused[0].contains("▸first"), "{focused:?}");

    let (unfocused, _) = render(&rows, false, 20, 6);
    assert!(!unfocused[0].contains('▸'), "{unfocused:?}");
    assert!(unfocused[0].contains("first"), "{unfocused:?}");
}

#[test]
fn a_heading_is_drawn_but_never_clickable() {
    let rows = [
        NavRow::heading("group"),
        NavRow::new("first"),
        NavRow::new("second"),
    ];

    let (lines, hits) = render(&rows, true, 20, 8);

    assert!(lines[0].contains("group"), "{lines:?}");
    // Two selectable rows, indexed 0 and 1 — the heading is absent, which is
    // what keeps a click on it from selecting whichever row shares its offset.
    assert_eq!(hits.rows.len(), 2);
    assert_eq!(hits.rows[0].1, 0);
    assert_eq!(hits.rows[1].1, 1);
    // And the heading's own row hits nothing.
    assert_eq!(hits.page_at(1, hits.area.y), None);
}

#[test]
fn nested_rows_are_indented_under_the_one_above_them() {
    let rows = [
        NavRow::new("workflow"),
        NavRow {
            indent: 2,
            ..NavRow::new("run")
        },
    ];

    let (lines, _) = render(&rows, false, 24, 6);

    assert!(lines[0].starts_with(" workflow"), "{lines:?}");
    assert!(lines[1].starts_with("   run"), "{lines:?}");
}

#[test]
fn the_viewport_follows_the_cursor_down_a_long_list() {
    let labels: Vec<String> = (0..30).map(|i| format!("row{i}")).collect();
    let mut rows: Vec<NavRow> = labels.iter().map(|l| NavRow::new(l)).collect();
    rows[25].selected = true;

    let (lines, hits) = render(&rows, true, 20, 8);

    assert!(
        lines.iter().any(|line| line.contains("row25")),
        "the selection must stay on screen: {lines:?}"
    );
    // Hit indices count from the top of the whole list, not the viewport, so a
    // click reports the same row whatever is scrolled into view.
    assert!(hits.rows.iter().any(|(_, index)| *index == 25));
}

#[test]
fn a_long_label_is_clipped_without_eating_the_indent() {
    // `clip` collapses runs of whitespace, so clipping the composed row rather
    // than just its label would flush every nested row against the border.
    let rows = [NavRow {
        indent: 2,
        ..NavRow::new("a run label far longer than this pane is wide")
    }];

    let (lines, _) = render(&rows, false, 20, 6);

    assert!(lines[0].starts_with("   a run"), "{lines:?}");
    assert!(lines[0].ends_with('…'), "{lines:?}");
    assert!(lines[0].chars().count() <= 20, "{lines:?}");
}

#[test]
fn a_sidebar_with_no_room_returns_no_hits_rather_than_panicking() {
    let rows = [NavRow::new("first")];
    let mut terminal = Terminal::new(TestBackend::new(8, 4)).expect("terminal");
    let mut hits = NavHits::default();
    terminal
        .draw(|f| {
            let empty = ratatui::layout::Rect {
                height: 0,
                ..f.area()
            };
            hits = draw_rows(
                f,
                empty,
                Block::default(),
                &Theme::default(),
                &rows,
                true,
                "hint",
                0,
            );
        })
        .expect("draw");
    assert!(hits.rows.is_empty());
}
