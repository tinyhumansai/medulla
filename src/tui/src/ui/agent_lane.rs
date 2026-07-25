//! Shared one-line agent-lane presentation.
//!
//! Both the orchestrator's Agents tab and the daemon's reduced Agents tab use
//! this formatter so presence glyphs, labels, suffix spacing, and selection
//! styling do not drift into two similar-looking components.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Render one agent lane from its marker, label, and already-formatted detail.
pub(crate) fn line(
    marker: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
    style: Style,
) -> Line<'static> {
    let detail = detail.into();
    Line::from(Span::styled(
        format!("{} {}{}", marker.into(), label.into(), detail),
        style,
    ))
}

#[cfg(test)]
#[path = "agent_lane_tests.rs"]
mod tests;
