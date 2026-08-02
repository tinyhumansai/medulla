//! Focused tests for compact workflow and colour-coded run labels.

use medulla::ui::workflows::WorkflowRow;
use medulla::workflows::RunStatus;
use ratatui::style::Color;

use super::super::super::workflows::WorkflowRailRow;
use super::rail::{rail_label, run_color, run_glyph};

fn row(label: &str, detail: &str) -> WorkflowRow {
    WorkflowRow {
        key: label.to_string(),
        label: label.to_string(),
        detail: detail.to_string(),
        degraded: false,
    }
}

#[test]
fn workflow_labels_do_not_repeat_metadata_beside_the_name() {
    let label = rail_label(&WorkflowRailRow::Workflow {
        index: 0,
        row: row("Nightly sweep", "manual · 3 steps · a long description"),
    });

    assert_eq!(label, "Nightly sweep");
}

#[test]
fn runs_use_traffic_light_colours_and_state_glyphs() {
    assert_eq!(run_color(RunStatus::Succeeded), Color::Green);
    assert_eq!(run_glyph(RunStatus::Succeeded), "✓");
    for status in [RunStatus::Running, RunStatus::PendingApproval] {
        assert_eq!(run_color(status), Color::Yellow);
        assert_eq!(run_glyph(status), "●");
    }
    for status in [
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert_eq!(run_color(status), Color::Red);
        assert_eq!(run_glyph(status), "✗");
    }
}
