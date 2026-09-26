//! Tests for the wordmark above the Sessions rail: that it rides in the rail's
//! own column, and that a short terminal spends its rows on the list instead.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::ui::app::App;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

/// Draw the whole Sessions tab and read the buffer back, one string per row.
fn rows(width: u16, height: u16) -> Vec<String> {
    let mut app = app();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| {
            app.draw_sessions_tab(f, Rect::new(0, 0, width, height));
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn the_wordmark_sits_above_the_rail_border() {
    let rows = rows(120, 30);

    for (offset, art) in crate::ui::LOGO.iter().enumerate() {
        assert!(
            rows[offset].starts_with(&format!(" {art}")),
            "row {offset} is {:?}, wanted the wordmark",
            rows[offset]
        );
    }
    // The rail's own border opens on the row right after the art, so the two
    // read as one column rather than as a banner over the whole tab.
    assert!(
        rows[crate::ui::LOGO.len()].starts_with('┌')
            || rows[crate::ui::LOGO.len()].starts_with('╭'),
        "row after the wordmark is {:?}, wanted the rail border",
        rows[crate::ui::LOGO.len()]
    );
}

#[test]
fn a_short_terminal_keeps_the_list_and_drops_the_wordmark() {
    let rows = rows(120, 8);

    assert!(
        !rows[0].trim_start().starts_with(crate::ui::LOGO[0].trim()),
        "row 0 is {:?}, wanted the rail rather than the wordmark",
        rows[0]
    );
}
