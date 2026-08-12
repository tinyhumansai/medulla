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
    let lines = app.own_session_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].to_string(), "● codex · unmanaged · main");
    assert_eq!(lines[1].to_string(), "  /workspace/medulla");
}

#[test]
fn a_renamed_thread_is_shown_on_its_own_default_line() {
    let app = app_with(StatusLineConfig::default());
    let mut row = harness_row("/workspace/medulla");
    row.thread_name = Some("Ship the sidebar".into());

    let lines = app.own_session_lines(&row, false, 48, NOW);

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
    let lines = app.own_session_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

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
    row.checkout.branch = Some("feat/a-very-long-branch-name-indeed".into());

    for width in [0, 1, 4, 8, 12, 36] {
        for line in app.own_session_lines(&row, false, width, NOW) {
            assert!(line.width() <= width, "width {width}: {line:?}");
        }
    }
}

#[test]
fn wide_branch_and_path_glyphs_stay_within_their_cell_budget() {
    let mut row = harness_row("/workspace/项目/medulla界面");
    row.checkout.branch = Some("功能/状态显示分支".into());

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
        let lines = app.own_session_lines(&row, false, 10, NOW);

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
    assert!(long.own_session_lines(&row, false, 48, NOW)[0]
        .to_string()
        .starts_with("● Codex · unmanaged"));

    let icons = app_with(StatusLineConfig {
        harness_style: HarnessNameStyle::Icon,
        control_style: ControlStyle::Icon,
        ..StatusLineConfig::default()
    });
    assert_eq!(
        icons.own_session_lines(&row, false, 48, NOW)[0].to_string(),
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
        app.own_session_lines(&row, false, 44, NOW)[0].to_string()
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
        app.own_session_lines(&row, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main"
    );
    assert_eq!(
        app.own_session_lines(&row, true, 48, NOW)[0].to_string(),
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
    let row =
        crate::ui::app::rail::RailRow::Session(Box::new(crate::ui::app::rail::SessionRailRow {
            agent_id: None,
            lane_index: None,
            task: None,
            local: Some(harness_row("/workspace/tinyhumans/products/medulla-public")),
            last: true,
        }));
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
        app.own_session_lines(&healthy, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main"
    );

    for state in [PtyState::Failed, PtyState::Exited { code: Some(2) }] {
        let mut alerting = harness_row("/workspace/medulla");
        alerting.state = state;
        assert!(
            app.own_session_lines(&alerting, false, 48, NOW)[0]
                .to_string()
                .ends_with("/workspace/medulla"),
            "{state:?} should count as an alert"
        );
    }

    let mut errored = harness_row("/workspace/medulla");
    errored.last_error = Some("spawn failed".into());
    assert!(app.own_session_lines(&errored, false, 48, NOW)[0]
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
        worktree: FieldPlacement::Hidden,
        path: FieldPlacement::Hidden,
        ..StatusLineConfig::default()
    });
    let lines = app.own_session_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 1, "the row must still occupy a clickable line");
    assert_eq!(lines[0].to_string(), "");
}

/// A row working in a linked worktree of the repository.
fn worktree_row(cwd: &str, worktree: &str, branch: &str) -> crate::worker::pty::SessionRow {
    let mut row = harness_row(cwd);
    row.checkout.worktree = Some(worktree.into());
    row.checkout.branch = Some(branch.into());
    row
}

#[test]
fn a_linked_worktree_is_named_ahead_of_the_branch() {
    let app = app_with(StatusLineConfig::default());
    let row = worktree_row("/workspace/worktrees/fix-login", "fix-login", "fix/login");

    let lines = app.own_session_lines(&row, false, 80, NOW);

    // Coarse fact first: which checkout, then what is checked out in it.
    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · ⑂ fix-login · fix/login · /workspace/worktrees/fix-login"
    );
}

#[test]
fn the_primary_checkout_draws_no_worktree_field_at_all() {
    let app = app_with(StatusLineConfig::default());
    // The same layout, on a session that is not in a linked worktree: the field
    // is configured to show and still costs the row nothing, which is what lets
    // it default to visible.
    let lines = app.own_session_lines(&harness_row("/workspace/medulla"), false, 64, NOW);

    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · main · /workspace/medulla"
    );
}

#[test]
fn hiding_the_worktree_field_drops_it_from_a_worktree_row() {
    let app = app_with(StatusLineConfig {
        worktree: FieldPlacement::Hidden,
        ..StatusLineConfig::default()
    });
    let row = worktree_row("/workspace/worktrees/fix-login", "fix-login", "fix/login");

    let line = app.own_session_lines(&row, false, 64, NOW)[0].to_string();

    assert!(!line.contains("fix-login ·"), "{line}");
    assert!(line.contains("fix/login"), "{line}");
}

#[test]
fn a_detached_head_is_named_by_its_commit_rather_than_left_blank() {
    // Otherwise a harness sitting on a checked-out review commit is
    // indistinguishable from one running outside Git entirely.
    let app = app_with(StatusLineConfig::default());
    let mut row = harness_row("/workspace/medulla");
    row.checkout.branch = None;
    row.checkout.head = Some("a1b2c3d".into());

    let lines = app.own_session_lines(&row, false, 64, NOW);

    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · @a1b2c3d · /workspace/medulla"
    );
}

#[test]
fn a_session_outside_git_still_draws_neither_branch_nor_worktree() {
    let app = app_with(StatusLineConfig::default());
    let mut row = harness_row("/tmp/scratch");
    row.checkout = Default::default();

    let lines = app.own_session_lines(&row, false, 64, NOW);

    assert_eq!(lines[0].to_string(), "● codex · unmanaged · /tmp/scratch");
}

#[test]
fn a_worktree_name_is_capped_like_a_branch_name() {
    let app = app_with(StatusLineConfig {
        path: FieldPlacement::Hidden,
        ..StatusLineConfig::default()
    });
    let row = worktree_row(
        "/workspace/worktrees/x",
        "a-very-long-worktree-name-indeed",
        "main",
    );

    let line = app.own_session_lines(&row, false, 96, NOW)[0].to_string();

    // Capped at the shared name budget, ellipsis included, so one worktree
    // cannot spend the whole row.
    assert!(line.contains("⑂ a-very-long-w…"), "{line}");
}

#[test]
fn a_narrow_row_keeps_the_branch_and_drops_the_worktree() {
    // The two are not equally perishable: a branch moves several times an hour
    // where a worktree is fixed for the life of the checkout, so the branch is
    // what a row too narrow for both must keep. A clipped `⑂ st…` would spend
    // the same columns saying only that a worktree exists.
    let app = app_with(StatusLineConfig::default());
    let row = worktree_row("/workspace/worktrees/fix-login", "fix-login", "fix/login");

    let narrow = app.own_session_lines(&row, false, 36, NOW)[0].to_string();
    assert!(narrow.contains("fix/"), "the branch survives: {narrow}");
    assert!(!narrow.contains('⑂'), "the worktree gives way: {narrow}");

    // Given the room, both are drawn.
    let wide = app.own_session_lines(&row, false, 80, NOW)[0].to_string();
    assert!(wide.contains("⑂ fix-login"), "{wide}");
    assert!(wide.contains("fix/login"), "{wide}");
}
