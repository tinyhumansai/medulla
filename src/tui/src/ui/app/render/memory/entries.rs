//! Full-width lists for directives, facets, and ranked search results.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::util::clip;

use super::super::super::types::{App, MemoryEntry};

impl App {
    /// Draw distilled persona directives as a full-width selectable list.
    pub(super) fn draw_memory_directives(&self, frame: &mut Frame, area: Rect) {
        let entries = self.memory_directive_entries();
        self.draw_memory_entry_list(
            frame,
            area,
            "Directives · Enter details",
            &entries,
            "No directives yet. Open Maintenance to backfill memory.",
        );
    }

    /// Draw observation facets as a full-width selectable list.
    pub(super) fn draw_memory_facets(&self, frame: &mut Frame, area: Rect) {
        let entries = self.memory_facet_entries();
        self.draw_memory_entry_list(
            frame,
            area,
            "Facets · Enter details",
            &entries,
            "No facets yet. Open Maintenance to backfill memory.",
        );
    }

    /// Draw the search query header and full-width ranked result list.
    pub(super) fn draw_memory_search(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(0)])
            .split(area);
        let query = self.memory_query.as_deref().unwrap_or("(none yet)");
        let header = vec![
            Line::from(format!("query: {}", clip(query, 72))),
            Line::from(Span::styled(
                "/ or q new search · ↑↓ browse · Enter details",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        frame.render_widget(
            Paragraph::new(Text::from(header)).block(self.panel("Search memory")),
            rows[0],
        );
        let entries: Vec<MemoryEntry> = self
            .memory_hits
            .iter()
            .cloned()
            .map(MemoryEntry::Hit)
            .collect();
        self.draw_memory_entry_list(
            frame,
            rows[1],
            &format!("Results · {} hit(s)", entries.len()),
            &entries,
            if self.memory_query.is_some() {
                "No hits for that query."
            } else {
                "Press / or q to search persona memory."
            },
        );
    }

    /// Render one active Memory collection without reserving a detail sidebar.
    fn draw_memory_entry_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        entries: &[MemoryEntry],
        empty: &str,
    ) {
        let block = self.panel(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let selected = self.memory_index.min(entries.len().saturating_sub(1));
        let visible = (inner.height as usize).max(1);
        let start = crate::ui::selection::viewport_start(selected, entries.len(), visible);
        let mut lines = Vec::new();
        for (index, entry) in entries.iter().enumerate().skip(start).take(visible) {
            let (label, base) = match entry {
                MemoryEntry::Directive(text) => (
                    format!("◆ {}", clip(text, inner.width.saturating_sub(4) as usize)),
                    Style::default().fg(Color::Yellow),
                ),
                MemoryEntry::Facet { name, count } => (
                    format!("▪ {name} · {count} observation(s)"),
                    Style::default().fg(self.theme.primary),
                ),
                MemoryEntry::Hit(hit) => (
                    format!(
                        "{} · {} · {:.2}  {}",
                        hit.facet,
                        hit.tier,
                        hit.score,
                        clip(&hit.text, inner.width.saturating_sub(24) as usize)
                    ),
                    Style::default().fg(Color::Magenta),
                ),
            };
            let style = if index == selected {
                self.theme.selection()
            } else {
                base
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
        if entries.is_empty() {
            lines.push(Line::from(Span::styled(
                empty.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
