//! Shared ratatui component chrome.
//!
//! Main and daemon screens use these constructors for panels and editable
//! prompt lines so visual roles come from one theme rather than hard-coded
//! colors in each application.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use super::composer::Draft;
use super::theme::Theme;

/// A rounded titled panel using the active theme.
pub(crate) fn panel<'a>(theme: &Theme, title: impl Into<String>, focused: bool) -> Block<'a> {
    let border = if focused {
        theme.primary
    } else {
        theme.dim_border
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title.into(),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ))
}

/// Render a single-line editable draft with a visible caret.
pub(crate) fn prompt_line(draft: &Draft, theme: &Theme) -> Line<'static> {
    let chars: Vec<char> = draft.text.chars().collect();
    let cursor = draft.cursor.min(chars.len());
    let before: String = chars.iter().take(cursor).collect();
    let at = chars
        .get(cursor)
        .map(char::to_string)
        .unwrap_or_else(|| " ".into());
    let after: String = chars.iter().skip(cursor + 1).collect();
    Line::from(vec![
        Span::styled("❯ ", Style::default().fg(theme.accent)),
        Span::raw(before),
        Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_line_clamps_the_caret() {
        let line = prompt_line(
            &Draft {
                text: "hi".into(),
                cursor: 99,
            },
            &Theme::default(),
        );
        assert_eq!(line.spans[1].content, "hi");
        assert_eq!(line.spans[2].content, " ");
    }
}
