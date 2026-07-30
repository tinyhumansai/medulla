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

mod types;

pub(crate) use types::{NavAction, NavHits, NavRow};

#[cfg(test)]
mod tests;

/// Default width of the subpage navigation column.
pub(crate) const NAV_WIDTH: u16 = 20;

/// Split an area into the standard subpage menu and content panes.
pub(crate) fn split(area: Rect) -> (Rect, Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(NAV_WIDTH), Constraint::Min(0)])
        .split(area);
    (columns[0], columns[1])
}

/// The width a list sidebar should take for its content.
///
/// A percentage split reads badly at both ends: on an 80-column terminal 36%
/// leaves the transcript too narrow to wrap prose, and on a 200-column one it
/// hands half the screen to a column of short labels. So the sidebar is sized to
/// what it holds — the widest row plus the panel's own borders — and then held
/// between a floor that keeps labels legible and a ceiling of three tenths of the
/// screen so a long workspace path can never crowd out the content beside it.
pub(crate) fn sidebar_width(total: u16, widest_row: usize) -> u16 {
    const FLOOR: u16 = 18;
    const BORDERS: u16 = 2;
    let ceiling = (total.saturating_mul(3) / 10).max(FLOOR);
    let wanted = (widest_row as u16).saturating_add(BORDERS);
    wanted.clamp(FLOOR, ceiling).min(total.saturating_sub(20))
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
///
/// Returns each page row's `(screen_row, page_index)` so the caller can hit-test
/// clicks against it. Group headings occupy rows of their own and are absent
/// from the result, which is what keeps a click on a heading inert instead of
/// selecting whichever page happens to share its offset.
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
) -> NavHits {
    // The leading column exists to sit pages under their heading. A flat menu
    // has nothing to sit under, so indenting it only pushes every row a column
    // away from the border for no reason.
    let pad = usize::from(!groups.is_empty());
    let mut rows: Vec<NavRow> = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        if let Some((heading, _)) = groups.iter().find(|(_, start)| *start == index) {
            rows.push(NavRow::heading(heading));
        }
        rows.push(NavRow {
            jump: Some(index + 1),
            indent: pad,
            selected: index == selected,
            ..NavRow::new(page)
        });
    }
    let hint = if focused {
        "Esc menu".to_string()
    } else {
        format!("↑↓ nav · ⏎ open · 1-{} jump", pages.len())
    };
    // `focused` here means the *content* pane holds the keyboard — the sense
    // this function has always been called with. `draw_rows` asks about the
    // menu, so it is the negation.
    draw_rows(f, area, block, theme, &rows, !focused, &hint, pad)
}

/// Draw an arbitrary list of [`NavRow`]s as a navigation sidebar.
///
/// The renderer behind [`draw_nav`], exposed because not every sidebar's rows
/// come from a fixed page list — the Workflows catalogue reads its own from the
/// store, and nesting a workflow's runs under it needs a row shape a `&[&str]`
/// cannot express.
///
/// The viewport follows the cursor, so a catalogue longer than the pane keeps
/// the selection on screen rather than the top of the list. Returns each
/// selectable row's `(screen_row, index)` for click hit-testing, where the index
/// counts selectable rows only — headings are drawn but never hit.
///
/// `nav_focused` asks whether *this menu* holds the keyboard — note that
/// [`draw_nav`]'s own `focused` parameter asks the opposite question, about the
/// content pane, and negates before calling here. The two sidebars in the app
/// had disagreed about which state gets the `▸` marker and which gets the
/// highlight; unified here so the marker always follows the keyboard.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_rows(
    f: &mut Frame,
    area: Rect,
    block: Block<'_>,
    theme: &Theme,
    rows: &[NavRow<'_>],
    nav_focused: bool,
    hint: &str,
    pad: usize,
) -> NavHits {
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return NavHits::default();
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let width = inner.width as usize;

    // One row is spent on the hint and one on the blank above it, so the list
    // gets what remains.
    let capacity = (inner.height as usize).saturating_sub(2).max(1);
    let cursor = rows.iter().position(|row| row.selected).unwrap_or(0);
    let start = crate::ui::selection::viewport_start(cursor, rows.len(), capacity);

    let mut lines = Vec::new();
    let mut hits = Vec::new();
    // Counts selectable rows from the top of the whole list, not the viewport,
    // so a click reports the same index whatever is scrolled into view.
    let mut selectable = rows.iter().take(start).filter(|row| row.selectable).count();
    for row in rows.iter().skip(start).take(capacity) {
        if row.selectable {
            // Recorded before the line is pushed: `lines.len()` is its offset
            // from the top of the inner area, headings included.
            hits.push((inner.y + lines.len() as u16, selectable));
            selectable += 1;
        }
        let style = match (row.selected, nav_focused, row.dim) {
            (true, true, _) => theme.selection(),
            // Bold rather than the selection highlight: the cursor is still
            // here, but the keyboard is in the content pane beside it.
            (true, false, _) => Style::default().add_modifier(Modifier::BOLD),
            (false, _, true) => dim,
            _ => Style::default(),
        };
        let marker = if row.selected && nav_focused {
            "▸"
        } else {
            " "
        };
        let jump = row.jump.map(|n| format!("{n} ")).unwrap_or_default();
        let prefix = format!("{}{marker}{jump}", " ".repeat(row.indent));
        // Only the label is clipped. `clip` collapses runs of whitespace, so
        // clipping the whole composed row would eat the indent that says a run
        // belongs to the workflow above it — and every nested row would sit
        // flush against the border.
        let text = format!(
            "{prefix}{}",
            crate::ui::util::clip(row.label, width.saturating_sub(prefix.chars().count()))
        );
        lines.push(TLine::from(Span::styled(text, style)));
    }
    lines.push(TLine::from(""));
    // Same reason the row above clips only its label: the pad would not survive
    // `clip`'s whitespace collapse.
    let pad = " ".repeat(pad);
    lines.push(TLine::from(Span::styled(
        format!(
            "{pad}{}",
            crate::ui::util::clip(hint, width.saturating_sub(pad.chars().count()))
        ),
        dim,
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
    NavHits {
        area: inner,
        rows: hits,
    }
}
