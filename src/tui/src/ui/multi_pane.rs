//! Reusable two-pane subpage navigation for TUI sections.
//!
//! Settings and Routing both present a narrow page selector beside a larger
//! content pane. This module owns that shared layout, navigation rendering, and
//! focus transitions so future sections only supply page names, optional group
//! headings, and their page-specific content/actions.

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::ui::theme::Theme;

#[cfg(test)]
mod tests;

/// Default width of the subpage navigation column.
pub(crate) const NAV_WIDTH: u16 = 20;

/// What shared navigation did with one key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavAction {
    /// The active page changed.
    SelectionChanged,
    /// Focus moved from the menu into the content pane.
    Entered,
    /// Focus moved from the content pane back to the menu.
    Left,
    /// The key belongs to the current mode but changed no state.
    Consumed,
    /// Page-specific or global handling should receive the key.
    Unhandled,
}

/// Split an area into the standard subpage menu and content panes.
pub(crate) fn split(area: Rect) -> (Rect, Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(NAV_WIDTH), Constraint::Min(0)])
        .split(area);
    (columns[0], columns[1])
}

/// Apply shared digit, arrow, Enter, and Escape navigation.
///
/// When the menu has focus, character keys are consumed so an accidental page
/// action cannot fire through the menu. When content has focus, page-specific
/// keys (including arrows) are left to the caller.
pub(crate) fn navigate(
    code: KeyCode,
    page_count: usize,
    selected: &mut usize,
    focused: &mut bool,
    allow_leave: bool,
) -> NavAction {
    if let KeyCode::Char(d @ '1'..='9') = code {
        let index = d as usize - '1' as usize;
        if index < page_count {
            *selected = index;
            *focused = true;
            return NavAction::SelectionChanged;
        }
        return NavAction::Unhandled;
    }

    if !*focused {
        return match code {
            KeyCode::Up | KeyCode::Down => {
                let previous = *selected;
                *selected = if matches!(code, KeyCode::Up) {
                    selected.saturating_sub(1)
                } else {
                    (*selected + 1).min(page_count.saturating_sub(1))
                };
                if previous == *selected {
                    NavAction::Consumed
                } else {
                    NavAction::SelectionChanged
                }
            }
            KeyCode::Enter => {
                *focused = true;
                NavAction::Entered
            }
            KeyCode::Char(_) => NavAction::Consumed,
            _ => NavAction::Unhandled,
        };
    }

    if code == KeyCode::Esc && allow_leave {
        *focused = false;
        NavAction::Left
    } else {
        NavAction::Unhandled
    }
}

/// Draw a standard subpage navigation panel.
///
/// `groups` contains display-only headings as `(label, first_page_index)`.
/// Passing an empty slice renders a flat menu.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_nav(
    f: &mut Frame,
    area: Rect,
    block: Block<'_>,
    theme: &Theme,
    pages: &[&str],
    groups: &[(&str, usize)],
    selected: usize,
    focused: bool,
) {
    let inner = block.inner(area);
    f.render_widget(block, area);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        if let Some((heading, _)) = groups.iter().find(|(_, start)| *start == index) {
            lines.push(TLine::from(Span::styled(format!(" {heading}"), dim)));
        }
        let style = match (index == selected, focused) {
            (true, false) => theme.selection(),
            (true, true) => Style::default().add_modifier(Modifier::BOLD),
            _ => Style::default(),
        };
        let marker = if index == selected && focused {
            "▸"
        } else {
            " "
        };
        lines.push(TLine::from(Span::styled(
            format!(" {marker}{} {page} ", index + 1),
            style,
        )));
    }
    lines.push(TLine::from(""));
    lines.push(TLine::from(Span::styled(
        if focused {
            " Esc menu".to_string()
        } else {
            format!(" ↑↓ nav · ⏎ open · 1-{} jump", pages.len())
        },
        dim,
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}
