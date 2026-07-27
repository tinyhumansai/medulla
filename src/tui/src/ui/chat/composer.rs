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

use crate::ui::composer::{caret_row_col, Draft};
use crate::ui::theme::Theme;

use super::types::ComposerChrome;

/// Rows a composer needs to show `text` in full, borders included.
///
/// Grows with the draft rather than capping it: a composer that stops growing
/// hides the end of what is being typed, which is the part being worked on.
pub(crate) fn composer_height(text: &str) -> u16 {
    const BORDERS: u16 = 2;
    (text.split('\n').count() as u16)
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
        Paragraph::new(Text::from(draft_lines(draft, theme, chrome.focused))),
        inner,
    );
}

/// The draft as one styled line per row, with the caret marked on its own row.
fn draft_lines(draft: &Draft, theme: &Theme, focused: bool) -> Vec<TLine<'static>> {
    let caret = caret_row_col(&draft.text, draft.cursor);
    let mut lines: Vec<TLine> = Vec::new();
    for (index, row) in draft.text.split('\n').enumerate() {
        // Only the first row is prompted. A continuation carrying `❯ ` too would
        // read as a second message rather than the same one wrapped.
        let prefix = if index == 0 { "❯ " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(theme.primary))];
        if index == caret.row {
            let chars: Vec<char> = row.chars().collect();
            let before: String = chars.iter().take(caret.col).collect();
            // Past the end of the row there is no character to reverse, so the
            // caret is drawn over a space — which is where the next one lands.
            let at: String = chars
                .get(caret.col)
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".into());
            let after: String = chars.iter().skip(caret.col + 1).collect();
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
            spans.push(Span::raw(row.to_string()));
        }
        lines.push(TLine::from(spans));
    }
    lines
}
