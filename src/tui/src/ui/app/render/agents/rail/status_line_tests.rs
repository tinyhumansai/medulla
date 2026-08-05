//! Focused tests for configurable harness status-line layout and visibility.

use crate::ui::app::App;
use crate::worker::pty::PtyState;
use medulla::config::{
    ControlStyle, FieldPlacement, FieldVisibility, HarnessNameStyle, PathStyle, StatusLineConfig,
};

use super::tests::{app, harness_row, NOW};

/// Build an app with an explicit status-line layout applied.
fn app_with(cfg: StatusLineConfig) -> App {
    let mut app = app();
    app.loaded.config.status_line = Some(cfg);
    app
}

#[test]
fn a_field_moved_to_line_two_leaves_the_first_line_and_indents() {
    let app = app_with(StatusLineConfig {
        path: FieldPlacement::Line2,
        ..StatusLineConfig::default()
    });
    let lines = app.own_harness_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].to_string(), "● codex · unmanaged · main");
    assert_eq!(lines[1].to_string(), "  /workspace/medulla");
}

#[test]
fn a_renamed_thread_is_shown_on_its_own_default_line() {
    let app = app_with(StatusLineConfig::default());
    let mut row = harness_row("/workspace/medulla");
    row.thread_name = Some("Ship the sidebar".into());

    let lines = app.own_harness_lines(&row, false, 48, NOW);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · main · /workspace/medulla"
    );
    // The harness advertises a sentence; the rail shows its slug.
    assert_eq!(lines[1].to_string(), "  ship-sidebar");
}

#[test]
fn three_lines_are_available_and_an_unused_one_is_closed_up() {
    let app = app_with(StatusLineConfig {
        branch: FieldPlacement::Line3,
        ..StatusLineConfig::default()
    });
    let lines = app.own_harness_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · /workspace/medulla"
    );
    assert_eq!(lines[1].to_string(), "  main");
}

#[test]
fn every_line_is_still_bounded_by_the_rail_width() {
    let app = app_with(StatusLineConfig {
        harness_style: HarnessNameStyle::Long,
        branch: FieldPlacement::Line2,
        path: FieldPlacement::Line3,
        path_style: PathStyle::Full,
        ..StatusLineConfig::default()
    });
    let mut row = harness_row("/workspace/tinyhumans/products/medulla-public");
    row.branch = Some("feat/a-very-long-branch-name-indeed".into());

    for width in [0, 1, 4, 8, 12, 36] {
        for line in app.own_harness_lines(&row, false, width, NOW) {
            assert!(line.width() <= width, "width {width}: {line:?}");
        }
    }
}

#[test]
fn wide_branch_and_path_glyphs_stay_within_their_cell_budget() {
    let mut row = harness_row("/workspace/项目/medulla界面");
    row.branch = Some("功能/状态显示分支".into());

    for path_style in [PathStyle::Full, PathStyle::Last] {
        let app = app_with(StatusLineConfig {
            state: FieldPlacement::Hidden,
            harness: FieldPlacement::Hidden,
            control: FieldPlacement::Hidden,
            branch: FieldPlacement::Line1,
            path: FieldPlacement::Line2,
            path_style,
            ..StatusLineConfig::default()
        });
        let lines = app.own_harness_lines(&row, false, 10, NOW);

        assert_eq!(lines.len(), 2);
        assert!(
            lines.iter().all(|line| line.width() <= 10),
            "{path_style:?}: {lines:?}"
        );
        assert!(lines.iter().all(|line| line.to_string().contains('…')));
    }
}

#[test]
fn the_harness_name_and_control_state_have_compact_spellings() {
    let row = harness_row("/workspace/medulla");
    let long = app_with(StatusLineConfig {
        harness_style: HarnessNameStyle::Long,
        ..StatusLineConfig::default()
    });
    assert!(long.own_harness_lines(&row, false, 48, NOW)[0]
        .to_string()
        .starts_with("● Codex · unmanaged"));

    let icons = app_with(StatusLineConfig {
        harness_style: HarnessNameStyle::Icon,
        control_style: ControlStyle::Icon,
        ..StatusLineConfig::default()
    });
    assert_eq!(
        icons.own_harness_lines(&row, false, 48, NOW)[0].to_string(),
        "● ◆ · ⊘ · main · /workspace/medulla"
    );
}

#[test]
fn the_path_style_chooses_how_much_of_the_directory_survives() {
    let row = harness_row("/workspace/tinyhumans/products/medulla-public");
    let with_style = |style| {
        let app = app_with(StatusLineConfig {
            state: FieldPlacement::Hidden,
            harness: FieldPlacement::Hidden,
            control: FieldPlacement::Hidden,
            branch: FieldPlacement::Hidden,
            path_style: style,
            ..StatusLineConfig::default()
        });
        app.own_harness_lines(&row, false, 44, NOW)[0].to_string()
    };

    assert_eq!(with_style(PathStyle::Last), "medulla-public");
    assert_eq!(
        with_style(PathStyle::Full),
        "…orkspace/tinyhumans/products/medulla-public",
        "Full spells the whole path, losing only what will not fit — from the head"
    );
    assert!(
        with_style(PathStyle::Shortened).ends_with("medulla-public"),
        "Shortened keeps the checkout name"
    );
}

#[test]
fn a_field_can_be_held_back_until_its_row_is_selected() {
    let app = app_with(StatusLineConfig {
        path_when: FieldVisibility::Active,
        ..StatusLineConfig::default()
    });
    let row = harness_row("/workspace/medulla");

    assert_eq!(
        app.own_harness_lines(&row, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main"
    );
    assert_eq!(
        app.own_harness_lines(&row, true, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main · /workspace/medulla"
    );
}

#[test]
fn rail_measurement_includes_fields_visible_only_on_the_selected_row() {
    let app = app_with(StatusLineConfig {
        state: FieldPlacement::Hidden,
        harness: FieldPlacement::Hidden,
        control: FieldPlacement::Hidden,
        branch: FieldPlacement::Hidden,
        path_when: FieldVisibility::Active,
        ..StatusLineConfig::default()
    });
    let row = crate::ui::app::rail::RailRow::Harness(harness_row(
        "/workspace/tinyhumans/products/medulla-public",
    ));
    let measured = app.rail_row_measurement_lines(&row, &[]);

    assert!(measured.iter().any(|line| line.width() == 0));
    assert!(
        measured
            .iter()
            .any(|line| line.to_string().ends_with("medulla-public")),
        "the active-only path must participate in measurement: {measured:?}"
    );
}

#[test]
fn an_on_alert_field_appears_only_for_a_harness_that_needs_attention() {
    let app = app_with(StatusLineConfig {
        path_when: FieldVisibility::Alert,
        ..StatusLineConfig::default()
    });
    let healthy = harness_row("/workspace/medulla");

    assert_eq!(
        app.own_harness_lines(&healthy, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main"
    );

    for state in [PtyState::Failed, PtyState::Exited { code: Some(2) }] {
        let mut alerting = harness_row("/workspace/medulla");
        alerting.state = state;
        assert!(
            app.own_harness_lines(&alerting, false, 48, NOW)[0]
                .to_string()
                .ends_with("/workspace/medulla"),
            "{state:?} should count as an alert"
        );
    }

    let mut errored = harness_row("/workspace/medulla");
    errored.last_error = Some("spawn failed".into());
    assert!(app.own_harness_lines(&errored, false, 48, NOW)[0]
        .to_string()
        .ends_with("/workspace/medulla"));
}

#[test]
fn hiding_every_field_still_leaves_one_selectable_line() {
    let app = app_with(StatusLineConfig {
        state: FieldPlacement::Hidden,
        harness: FieldPlacement::Hidden,
        control: FieldPlacement::Hidden,
        branch: FieldPlacement::Hidden,
        path: FieldPlacement::Hidden,
        ..StatusLineConfig::default()
    });
    let lines = app.own_harness_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 1, "the row must still occupy a clickable line");
    assert_eq!(lines[0].to_string(), "");
}
