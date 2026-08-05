//! What the live pane makes of a step's streamed harness frames.

use medulla::daemon::{THINKING_PREFIX, TOOL_PREFIX};

use super::live::live_lines;

/// Flatten styled lines into the text an operator reads.
fn text(lines: Vec<ratatui::text::Line<'static>>) -> String {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_step_with_no_frames_draws_nothing_at_all() {
    // Not an empty box with a heading: a step that has said nothing should
    // cost the preview no rows.
    assert!(live_lines(&[], true).is_empty());
}

#[test]
fn frames_are_marked_up_as_the_kinds_of_frame_they_are() {
    let frames = vec![
        format!("{TOOL_PREFIX}bash: cargo test"),
        "tool completed · exit 0".to_string(),
        format!("{THINKING_PREFIX}weighing the failure"),
        "writing".to_string(),
    ];

    let preview = text(live_lines(&frames, true));

    assert!(preview.contains("live"), "{preview}");
    assert!(preview.contains("⏺ bash: cargo test"), "{preview}");
    assert!(preview.contains("✓ exit 0"), "{preview}");
    assert!(preview.contains("weighing the failure"), "{preview}");
    assert!(preview.contains("writing"), "{preview}");
    // The producer's prefixes are machinery, not something to read.
    assert!(!preview.contains(TOOL_PREFIX), "{preview}");
}

#[test]
fn a_settled_run_says_its_frames_are_the_last_ones_rather_than_live() {
    let preview = text(live_lines(&["writing".to_string()], false));

    assert!(preview.contains("last"), "{preview}");
    assert!(!preview.contains(" live "), "{preview}");
}

#[test]
fn a_long_stream_reports_what_it_dropped_instead_of_silently_clipping() {
    let frames: Vec<String> = (0..60).map(|index| format!("step {index}")).collect();

    let preview = text(live_lines(&frames, true));

    assert!(preview.contains("20 earlier frames"), "{preview}");
    assert!(preview.contains("step 59"), "{preview}");
    assert!(!preview.contains("step 0\n"), "{preview}");
}
