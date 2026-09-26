//! The chat composer: a bordered, focus-aware input with a real caret.
//!
//! Free functions over a [`Draft`] and a [`Theme`] rather than a method on
//! `App`, so the two surfaces that draw one — the orchestrator and the workflow
//! copilot — get the same widget instead of the same idea implemented twice.
//!
//! The border is the focus affordance. Its colour answers "is this thing
//! working?" (yellow) before "does it have my keystrokes?" (the theme's primary
//! against a dim border), because a busy surface is the state an operator most
//! needs to read at a glance.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::composer::{caret_visual, wrap_rows, Draft};
use crate::ui::theme::Theme;

use super::types::ComposerChrome;

/// Columns the prompt gutter (`❯ ` and its continuation indent) claims on every
/// row, and so cannot be spent on text.
const GUTTER: u16 = 2;

/// Columns the border claims, one either side.
const BORDERS: u16 = 2;

/// Text columns a composer drawn into `width` actually has.
fn text_width(width: u16) -> u16 {
    width.saturating_sub(BORDERS + GUTTER)
}

/// Rows a composer `width` columns wide needs to show `text` in full, borders
/// included.
///
/// Counts *rendered* rows, not hard newlines: a paragraph with no `\n` in it
/// still occupies several rows once it passes the pane's right edge, and a
/// height that ignored that would clip the part being typed.
///
/// Grows with the draft rather than capping it: a composer that stops growing
/// hides the end of what is being typed, which is the part being worked on.
/// Callers with a pane to protect clamp it themselves — [`draw_composer`]
/// scrolls to the caret when they do.
pub(crate) fn composer_height(text: &str, width: u16) -> u16 {
    let columns = text_width(width);
    if columns == 0 {
        // Too narrow to lay text out at all. Reporting a wrapped height here
        // would ask for one row per char of a draft that cannot be drawn.
        return 1 + BORDERS;
    }
    (wrap_rows(text, columns as usize).len() as u16)
        .max(1)
        .saturating_add(BORDERS)
}

/// Draw a composer for `draft` into `area`.
///
/// The caret is drawn solid only while the composer holds the keyboard. When
/// focus has stepped away a reversed block would still read as "your typing goes
/// here", which is exactly the thing that is no longer true — two visible carets
/// and no way to tell which one is live. It is drawn dim instead, so the
/// insertion point is still findable when focus comes back.
///
/// An empty unfocused draft shows `chrome.placeholder` when there is one. A
/// focused composer never shows it: the caret is the invitation, and a
/// placeholder under it reads as text that is already there.
pub(crate) fn draw_composer(
    f: &mut Frame,
    area: Rect,
    draft: &Draft,
    theme: &Theme,
    chrome: ComposerChrome<'_>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if chrome.busy {
            Color::Yellow
        } else if chrome.focused {
            theme.primary
        } else {
            theme.dim_border
        }));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if let Some(placeholder) = chrome.placeholder {
        if draft.text.is_empty() && !chrome.focused {
            f.render_widget(
                Paragraph::new(TLine::from(vec![
                    Span::styled("❯ ", Style::default().fg(theme.dim_border)),
                    Span::styled(
                        placeholder.to_string(),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])),
                inner,
            );
            return;
        }
    }

    f.render_widget(
        Paragraph::new(Text::from(draft_lines(
            draft,
            theme,
            chrome.focused,
            text_width(area.width),
            inner.height,
        ))),
        inner,
    );
}

/// The draft as one styled line per rendered row, with the caret marked on its
/// own row, clipped to the `height` rows the pane granted.
///
/// When the draft is taller than that — a caller capping the composer so it
/// cannot eat the transcript above it — the window follows the caret rather than
/// staying at the top, because the row being typed on is the one that has to be
/// on screen.
fn draft_lines(
    draft: &Draft,
    theme: &Theme,
    focused: bool,
    width: u16,
    height: u16,
) -> Vec<TLine<'static>> {
    let rows = wrap_rows(&draft.text, width.max(1) as usize);
    let (caret_row, caret_col) = caret_visual(&rows, draft.cursor);
    let visible = (height as usize).max(1);
    let start = if rows.len() <= visible {
        0
    } else {
        caret_row
            .saturating_sub(visible - 1)
            .min(rows.len() - visible)
    };

    let mut lines: Vec<TLine> = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
        // Only the very first row is prompted. A continuation carrying `❯ ` too
        // would read as a second message rather than the same one wrapped.
        let prefix = if index == 0 { "❯ " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(theme.primary))];
        if index == caret_row {
            let chars: Vec<char> = row.text.chars().collect();
            let before: String = chars.iter().take(caret_col).collect();
            // Past the end of the row there is no character to reverse, so the
            // caret is drawn over a space — which is where the next one lands.
            let at: String = chars
                .get(caret_col)
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".into());
            let after: String = chars.iter().skip(caret_col + 1).collect();
            spans.push(Span::raw(before));
            spans.push(Span::styled(
                at,
                if focused {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                },
            ));
            spans.push(Span::raw(after));
        } else {
            spans.push(Span::raw(row.text.clone()));
        }
        lines.push(TLine::from(spans));
    }
    lines
}
