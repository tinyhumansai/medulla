//! Focused rendering tests for compact copilot tool activity.

use medulla::ui::workflows::{CopilotTurn, TurnRole};

use super::copilot::turn_lines;

/// Flatten one styled terminal line into the text an operator sees.
fn text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn a_long_tool_command_uses_at_most_two_rows() {
    let turn = CopilotTurn::new(
        TurnRole::Tool,
        "Terminal · $ cargo test --workspace --all-targets --all-features",
    );

    let lines = turn_lines(&turn, 32);
    let visible = lines.iter().map(text).collect::<Vec<_>>();

    assert_eq!(lines.len(), 2, "{visible:?}");
    assert!(visible[0].contains("↻ Terminal"), "{visible:?}");
    assert!(visible[1].contains("$ cargo test"), "{visible:?}");
    assert!(visible.iter().all(|line| line.chars().count() <= 32));
}

#[test]
fn a_settled_tool_keeps_its_action_and_shows_the_result() {
    let turn = CopilotTurn::new(
        TurnRole::ToolSuccess,
        "Read · src/sdk/Cargo.toml · 2.4 KiB output",
    );

    let visible = turn_lines(&turn, 80)
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(visible.contains("✓ Read"), "{visible}");
    assert!(visible.contains("src/sdk/Cargo.toml · 2.4 KiB output"));
}

#[test]
fn tool_summary_supports_title_only_and_colon_fallback_forms() {
    let title_only = turn_lines(&CopilotTurn::new(TurnRole::Tool, "Terminal"), 80)
        .iter()
        .map(text)
        .collect::<Vec<_>>();
    assert_eq!(title_only, ["↻ Terminal"]);

    let colon = turn_lines(
        &CopilotTurn::new(TurnRole::ToolSuccess, "Read: src/main.rs"),
        80,
    )
    .iter()
    .map(text)
    .collect::<Vec<_>>()
    .join("\n");
    assert!(colon.contains("✓ Read"), "{colon}");
    assert!(colon.contains("src/main.rs"), "{colon}");
}
