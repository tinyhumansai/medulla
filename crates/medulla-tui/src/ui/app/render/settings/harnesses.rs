//! The Harnesses Settings section and its injected, side-effect-free view data.

pub(super) use super::super::super::types::HarnessesView;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

impl super::super::super::types::App {
    /// Render cached facts, falling back to the effective configuration while collection runs.
    pub(super) fn draw_harnesses(&mut self, f: &mut Frame, area: Rect) {
        let view = self.harnesses_view.clone().unwrap_or_else(|| {
            super::super::super::harnesses::view_from_config(&self.loaded.config)
        });
        draw_harnesses(f, area, &view);
    }
}

/// Draw pre-collected provider and configuration facts without side effects.
pub(super) fn draw_harnesses(f: &mut Frame, area: Rect, view: &HarnessesView) {
    let mut lines = vec![Line::from("Detected providers")];
    if view.providers.is_empty() {
        lines.push(Line::from(if view.detection_complete {
            "No providers detected."
        } else {
            "Provider detection has not been sampled."
        }));
    } else {
        for provider in &view.providers {
            lines.push(Line::from(format!(
                "{} · {}",
                provider.name,
                provider.version.as_deref().unwrap_or("version unavailable")
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Routing"));
    if view.routing.is_empty() {
        lines.push(Line::from("Not configured."));
    } else {
        for (tier, model) in &view.routing {
            lines.push(Line::from(format!("{tier} → {model}")));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Budgets"));
    if view.budgets.is_empty() {
        lines.push(Line::from("No provider budgets configured."));
    } else {
        for (provider, budget) in &view.budgets {
            lines.push(Line::from(format!("{provider} · {budget}")));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Effective configuration"));
    if view.configuration.is_empty() {
        lines.push(Line::from("No configuration facts supplied."));
    } else {
        lines.extend(
            view.configuration
                .iter()
                .map(|item| Line::from(item.as_str())),
        );
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Managed customizations"));
    if view.live_facts.is_empty() {
        lines.push(Line::from("No managed customization facts supplied."));
    } else {
        lines.extend(view.live_facts.iter().map(|item| Line::from(item.as_str())));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Harnesses")),
        area,
    );
}
