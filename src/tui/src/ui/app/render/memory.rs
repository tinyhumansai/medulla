//! The Memory tab: the persona-memory status header, the directives/facets or
//! search-hit list, and the selected entry's detail pane. When memory is not
//! enabled it renders a single hint panel instead.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::util::clip;
use medulla::memory::MemoryStatus;

use super::super::types::{App, MemoryEntry};

impl App {
    /// Draw the Memory tab: status header, entry list, and detail pane (or a
    /// disabled-state hint when persona memory is off).
    pub(super) fn draw_memory(&mut self, f: &mut Frame, area: Rect) {
        // Disabled / not wired: a single helpful hint panel.
        let enabled = self
            .memory_status
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false);
        if !enabled {
            let mut lines = vec![TLine::from(Span::styled(
                "Persona memory is not enabled.",
                Style::default().fg(Color::Yellow),
            ))];
            lines.push(TLine::from(Span::styled(
                "Enable it in config (memory.enabled = true) with an OpenRouter key,",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(Span::styled(
                "then run `medulla memory backfill` to distil your persona pack.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            f.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: true })
                    .block(self.panel("Persona memory")),
                area,
            );
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(0)])
            .split(area);

        // Status header.
        let st = self.memory_status.clone().unwrap_or(MemoryStatus {
            enabled: true,
            workspace: String::new(),
            pack_exists: false,
            pack_path: String::new(),
            entry_count: 0,
            directives_count: 0,
            facet_counts: Default::default(),
        });
        let mut header = vec![
            TLine::from(vec![
                Span::styled("● enabled", Style::default().fg(Color::Green)),
                Span::raw(format!(" · {}", clip(&st.workspace, 48))),
            ]),
            if st.pack_exists {
                TLine::from(Span::styled(
                    format!("pack ● present · {}", clip(&st.pack_path, 52)),
                    Style::default().fg(Color::Green),
                ))
            } else {
                TLine::from(Span::styled(
                    "pack ○ absent · run `medulla memory backfill`",
                    Style::default().add_modifier(Modifier::DIM),
                ))
            },
            TLine::from(format!(
                "{} observation(s) · {} directive(s)",
                st.entry_count, st.directives_count
            )),
        ];
        let facets = if st.facet_counts.is_empty() {
            "facets: (none)".to_string()
        } else {
            let joined = st
                .facet_counts
                .iter()
                .map(|(f, n)| format!("{f}={n}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("facets: {joined}")
        };
        header.push(TLine::from(Span::styled(
            facets,
            Style::default().fg(self.theme.primary),
        )));
        f.render_widget(
            Paragraph::new(Text::from(header))
                .wrap(Wrap { trim: true })
                .block(self.panel("Persona memory")),
            rows[0],
        );

        // Left list + right detail.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(rows[1]);

        let entries = self.memory_entries();
        let idx = self.memory_index.min(entries.len().saturating_sub(1));
        let searching = self.memory_query.is_some();
        let left_title = match &self.memory_query {
            Some(q) => format!("Search “{}” · {} hit(s)", clip(q, 18), entries.len()),
            None => "Directives & facets".to_string(),
        };
        let block = self.panel(left_title);
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let vis = (inner.height as usize).max(1);
        let start = idx
            .saturating_sub(vis / 2)
            .min(entries.len().saturating_sub(vis));
        let mut lines: Vec<TLine> = Vec::new();
        for (i, entry) in entries.iter().enumerate().skip(start).take(vis) {
            let (label, base) = match entry {
                MemoryEntry::Directive(text) => (
                    format!("◆ {}", clip(text, 30)),
                    Style::default().fg(Color::Yellow),
                ),
                MemoryEntry::Facet { name, count } => (
                    format!("▪ {name} · {count}"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                MemoryEntry::Hit(hit) => (
                    format!("{} · {} · {:.2}", hit.facet, hit.tier, hit.score),
                    Style::default().fg(Color::Magenta),
                ),
            };
            let mut style = base;
            if i == idx {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(label, style)));
        }
        if entries.is_empty() {
            let hint = if searching {
                "No hits for that query."
            } else {
                "No directives or observations yet. Run `medulla memory backfill`."
            };
            lines.push(TLine::from(Span::styled(
                hint,
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Detail pane.
        let (title, body) = self.memory_detail(entries.get(idx));
        f.render_widget(
            Paragraph::new(Text::from(body))
                .wrap(Wrap { trim: false })
                .block(self.panel(title)),
            cols[1],
        );
    }

    /// The detail title + wrapped body for the selected Memory entry.
    pub(super) fn memory_detail(
        &self,
        entry: Option<&MemoryEntry>,
    ) -> (String, Vec<TLine<'static>>) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        match entry {
            None => (
                "Detail".into(),
                vec![TLine::from(Span::styled(
                    "Select an entry with ↑/↓ (or search with /memory <query>).",
                    dim,
                ))],
            ),
            Some(MemoryEntry::Directive(text)) => {
                ("Directive".into(), vec![TLine::from(text.clone())])
            }
            Some(MemoryEntry::Facet { name, count }) => (
                name.clone(),
                vec![
                    TLine::from(format!("{count} observation(s) in this facet.")),
                    TLine::from(Span::styled(
                        "Run /memory <query> to rank observations across facets.",
                        dim,
                    )),
                ],
            ),
            Some(MemoryEntry::Hit(hit)) => {
                let mut body = vec![TLine::from(hit.text.clone()), TLine::from("")];
                if let Some(q) = &hit.quote {
                    body.push(TLine::from(Span::styled(format!("“{q}”"), dim)));
                    body.push(TLine::from(""));
                }
                body.push(TLine::from(Span::styled(
                    format!(
                        "facet {} · tier {} · score {:.3}",
                        hit.facet, hit.tier, hit.score
                    ),
                    dim,
                )));
                body.push(TLine::from(Span::styled(hit.timestamp.clone(), dim)));
                (format!("{} · {}", hit.facet, hit.tier), body)
            }
        }
    }
}
