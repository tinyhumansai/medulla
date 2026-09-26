//! Shared terminal-layout helpers.
//!
//! Centered screens and popups use these functions so clamping and percentage
//! calculations remain identical across login, onboarding, welcome, task,
//! memory, and daemon overlays.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;

/// Center a fixed-size rectangle inside `area`, clamping it to the available
/// terminal dimensions.
pub(crate) fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(rows[1]);
    columns[1]
}

/// Center a percentage-sized rectangle inside `area`.
///
/// Percentages are clamped to `0..=100`, preventing underflow when values come
/// from future configuration rather than compile-time constants.
pub(crate) fn centered_percent(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let width = width_percent.min(100);
    let height = height_percent.min(100);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Percentage(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width) / 2),
            Constraint::Percentage(width),
            Constraint::Percentage((100 - width) / 2),
        ])
        .split(vertical[1])[1]
}

/// The panel width that fits `lines`: the widest row plus both borders, held
/// wide enough for the `" login "` title and never wider than the terminal.
pub(crate) fn panel_width(lines: &[Line<'_>], screen_width: u16) -> u16 {
    const TITLE: u16 = 11;
    const BORDERS: u16 = 2;
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    widest
        .saturating_add(BORDERS)
        .max(TITLE)
        .min(screen_width.max(1))
}

/// The panel height that fits `lines` at `width`, counting the extra rows a
/// too-long line costs once [`Wrap`] has folded it.
pub(crate) fn panel_height(lines: &[Line<'_>], width: u16, screen_height: u16) -> u16 {
    const BORDERS: u16 = 2;
    let inner = width.saturating_sub(BORDERS).max(1);
    let rows: u16 = lines
        .iter()
        .map(|line| ((line.width() as u16).div_ceil(inner)).max(1))
        .sum();
    rows.saturating_add(BORDERS).min(screen_height.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_layout_centers_and_clamps() {
        let area = Rect::new(5, 7, 40, 20);
        assert_eq!(centered_fixed(20, 10, area), Rect::new(15, 12, 20, 10));
        assert_eq!(centered_fixed(80, 40, area), area);
    }

    #[test]
    fn percentage_layout_centers_and_clamps() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(centered_percent(area, 60, 50), Rect::new(20, 10, 60, 20));
        assert_eq!(centered_percent(area, 200, 200), area);
    }
}
