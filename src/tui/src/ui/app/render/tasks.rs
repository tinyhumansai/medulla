//! Rendering for the Tasks tab's task-list and source-management pages.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::{TASKS_SUBPAGES, TP_SOURCES, TP_TASKS};
use super::super::App;
use crate::ui::multi_pane;
use crate::ui::util::clip;

impl App {
    /// Render the Tasks menu and its active content page.
    pub(super) fn draw_tasks(&mut self, frame: &mut Frame, area: Rect) {
        let (nav, content) = multi_pane::split(area);
        multi_pane::draw_nav(
            frame,
            nav,
            self.panel("Tasks"),
            &self.theme,
            &TASKS_SUBPAGES,
            &[],
            self.tasks_index,
            self.tasks_focused,
        );
        match self.tasks_index {
            TP_TASKS => self.draw_task_list(frame, content),
            TP_SOURCES => self.draw_task_sources(frame, content),
            _ => self.draw_task_list(frame, content),
        }
        if self.tasks_detail_open {
            self.draw_task_detail_popup(frame, area);
        }
    }

    /// Render local tasks in a full-width selectable list.
    fn draw_task_list(&mut self, frame: &mut Frame, area: Rect) {
        let mut list = Vec::new();
        if self.tasks.tasks.is_empty() {
            list.push(Line::from(Span::styled(
                "No tasks yet · add tasks in the local repository",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        for (index, task) in self.tasks.tasks.iter().enumerate() {
            let selected = index == self.selected.min(self.tasks.tasks.len().saturating_sub(1));
            let style = if selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let status_style = if selected {
                style
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            list.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, style),
                Span::styled(clip(&task.title, 28), style),
                Span::styled(format!("  [{:?}]", task.status), status_style),
            ]));
        }
        frame.render_widget(
            Paragraph::new(list)
                .block(self.panel("All Tasks · a add · e edit · d delete · Enter details")),
            area,
        );
    }

    /// Render configured external task providers in a full-width selectable list.
    fn draw_task_sources(&mut self, frame: &mut Frame, area: Rect) {
        let mut rows = Vec::new();
        for (index, source) in self.tasks.sources.iter().enumerate() {
            let selected = index
                == self
                    .task_source_index
                    .min(self.tasks.sources.len().saturating_sub(1));
            let style = if selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let state = if source.enabled {
                "enabled"
            } else {
                "disabled"
            };
            rows.push(Line::from(Span::styled(
                format!(
                    "{}{} · {}",
                    if selected { "▸ " } else { "  " },
                    clip(&source.repository, 30),
                    state
                ),
                style,
            )));
        }
        if rows.is_empty() {
            rows.push(Line::from(Span::styled(
                "No sources configured · press a to add GitHub",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        frame.render_widget(
            Paragraph::new(rows).block(self.panel("Sources · a add · s sync · Enter details")),
            area,
        );
    }

    /// Draw selected task or source details over the Tasks page.
    fn draw_task_detail_popup(&self, frame: &mut Frame, area: Rect) {
        let detail = match self.tasks_index {
            TP_TASKS => self
                .tasks
                .tasks
                .get(self.selected.min(self.tasks.tasks.len().saturating_sub(1)))
                .map(|task| {
                    let source = task
                        .source
                        .as_ref()
                        .map(|source| format!("{}:{}", source.provider, source.source_id))
                        .unwrap_or_else(|| "local".into());
                    (
                        task.title.clone(),
                        format!(
                            "{}\n\nstatus: {:?}\nsource: {}\ncreated: {}\nupdated: {}\nlast sync: {}\n\nEsc close",
                            task.description,
                            task.status,
                            source,
                            task.created_at,
                            task.updated_at,
                            task.last_synced_at.as_deref().unwrap_or("never"),
                        ),
                    )
                }),
            TP_SOURCES => self
                .tasks
                .sources
                .get(
                    self.task_source_index
                        .min(self.tasks.sources.len().saturating_sub(1)),
                )
                .map(|source| {
                    (
                        source.repository.clone(),
                        format!(
                            "provider: {}\nstate filter: {}\nlabels: {}\ncustom filter: {}\ncredentials: {}\n\nEsc close",
                            source.provider,
                            source.state,
                            if source.labels.is_empty() {
                                "(none)".into()
                            } else {
                                source.labels.join(", ")
                            },
                            source.filter.as_deref().unwrap_or("(none)"),
                            if source.token.is_some() {
                                "configured"
                            } else {
                                "GITHUB_TOKEN / default"
                            },
                        ),
                    )
                }),
            _ => None,
        };
        let Some((title, body)) = detail else {
            return;
        };
        let popup = crate::ui::layout::centered_percent(area, 70, 60);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .block(self.panel(title)),
            popup,
        );
    }
}
