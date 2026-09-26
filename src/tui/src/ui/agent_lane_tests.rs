//! Tests for the agent lane module.

use super::line;
use ratatui::style::Style;

#[test]
fn lane_spacing_is_shared_and_stable() {
    let rendered = line("●", "worker-1", " · busy", Style::default());
    assert_eq!(rendered.spans[0].content, "● worker-1 · busy");
}
