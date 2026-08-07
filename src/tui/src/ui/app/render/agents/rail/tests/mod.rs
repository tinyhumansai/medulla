//! Unit-test fixtures and focused coverage for the Agents rail.

mod workflow_run_tests;
mod wrap_tests;

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::agents::{AgentLane, AgentRole, AgentRow, TaskState, TaskStatus};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use crate::ui::app::App;
use crate::worker::pty::{AttentionKind, HarnessAttention, PtyState, SessionControl, SessionRow};

use super::rows::{display_session_title, lane_title, running_session_title};

pub(super) fn app() -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()))
}

/// A fixed "now" for row rendering, so an elapsed-time suffix in an assertion
/// does not depend on when the test ran.
pub(super) const NOW: i64 = 10_000;

#[test]
fn attention_uses_the_configured_color_and_can_stay_solid() {
    let mut app = app();
    app.theme.attention = Color::LightMagenta;
    app.theme.attention_blink = false;
    let mut row = harness_row("/workspace/medulla");
    row.attention = Some(HarnessAttention::new(
        AttentionKind::Approval,
        "codex is asking permission",
        0,
    ));

    let lines = app.own_session_lines(&row, false, 48, NOW);
    let style = lines[0].spans[0].style;

    assert_eq!(style.fg, Some(Color::LightMagenta));
    assert!(!style.add_modifier.contains(Modifier::SLOW_BLINK));
}

/// No harness is waiting, which is what most of these rows assume.
fn none_waiting() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}

fn lane() -> AgentLane {
    AgentLane {
        key: "k".into(),
        label: "worker".into(),
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

fn task(status: TaskStatus, attention: bool, at: i64) -> TaskState {
    TaskState {
        task_id: format!("task-{at}"),
        status,
        turns: 1,
        last_at: at,
        turn_blocks: Vec::new(),
        attention: attention.then(|| "confirm: continue?".to_string()),
        question_id: attention.then(|| "question-1".to_string()),
        work: None,
    }
}

#[test]
fn the_newest_running_harness_title_identifies_an_agent_lane() {
    let mut harness = lane();
    harness.tasks = vec![
        task(TaskStatus::Done, false, 1),
        task(TaskStatus::Running, false, 3),
        task(TaskStatus::Running, false, 2),
    ];

    let title = running_session_title(&harness, |task_id| match task_id {
        "task-1" => Some("stale title".into()),
        "task-2" => Some("older live title".into()),
        "task-3" => Some("Fix session titles".into()),
        _ => None,
    });

    assert_eq!(title.as_deref(), Some("Fix session titles"));
}

#[test]
fn session_titles_are_slugged_and_bounded_before_rail_wrapping() {
    let title = format!("first line\n{}", "wide title ".repeat(20));

    let displayed = display_session_title(&title);

    assert_eq!(displayed, "first-line-wide");
    assert!(UnicodeWidthStr::width(displayed.as_str()) <= 48);
    assert!(!displayed.contains('\n'));
}

#[test]
fn session_titles_of_wide_characters_stay_within_the_rails_cell_budget() {
    // 48 wide characters pass the slug's character ceiling untouched but would
    // occupy 96 columns, so the rail clips them a second time by cell width.
    let title = "界".repeat(48);

    let displayed = display_session_title(&title);

    assert!(UnicodeWidthStr::width(displayed.as_str()) <= 48);
    assert!(displayed.ends_with('…'));
}

#[test]
fn a_title_that_slugs_to_nothing_is_not_a_lane_title() {
    // Punctuation- and control-only titles leave the slug empty. Passing that
    // on would render a dangling " · " and, since the newest running task wins
    // the lane, would mask an older task that does advertise a real title.
    assert_eq!(lane_title("---"), None);
    assert_eq!(lane_title("  \n\t\u{1b}  "), None);
    assert_eq!(lane_title(""), None);
    // An escape sequence is not empty, though: the slug strips the control
    // bytes and keeps the alphanumerics, which is the safe outcome.
    assert_eq!(lane_title("\u{1b}[2J"), Some("2j".to_string()));
    // All-filler input is a different case: slug names it badly-but-stably
    // rather than emptily, so the lane keeps showing it.
    assert_eq!(lane_title("okay so the"), Some("okay-so-the".to_string()));

    assert_eq!(
        lane_title("Fix session titles"),
        Some("fix-session-titles".to_string())
    );
}

#[test]
fn session_titles_keep_three_words_of_a_harness_sentence() {
    assert_eq!(
        display_session_title("Fix session handoff flow and pointer"),
        "fix-session-handoff"
    );
}

#[test]
fn the_overflow_row_counts_what_is_hidden_and_offers_to_fold_when_nothing_is() {
    let app = app();
    let text = |hidden| {
        let row = AgentRow::More {
            lane_index: 0,
            hidden,
        };
        app.agent_row_line(&row, &[lane()], false, &none_waiting())
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>()
    };

    assert!(text(7).contains("+7 more"));
    // Fully revealed, the same row is the way back to one page.
    assert!(text(0).contains("show less"));
}

#[test]
fn the_overflow_row_highlights_under_the_cursor() {
    let app = app();
    let row = AgentRow::More {
        lane_index: 0,
        hidden: 3,
    };

    let selected = app.agent_row_line(&row, &[lane()], true, &none_waiting());
    let idle = app.agent_row_line(&row, &[lane()], false, &none_waiting());

    // It is a control, so it must show the cursor rather than staying dim the
    // way the `── functions ──` label does.
    assert_eq!(selected.spans[0].style, app.theme.selection());
    assert!(idle.spans[0].style.add_modifier.contains(Modifier::DIM));
}

pub(super) fn harness_row(cwd: &str) -> SessionRow {
    SessionRow {
        mcp_grant_session: None,
        id: "w_1".into(),
        label: "local".into(),
        provider: medulla::protocol::HarnessProvider::Codex,
        preset: None,
        state: PtyState::Running,
        cwd: cwd.into(),
        branch: Some("main".into()),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: None,
        started_at: 1,
        last_output_at: 1,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: crate::worker::pty::SessionOrigin::User,
        retained: false,
        name: None,
        attention: None,
        working: false,
    }
}

#[test]
fn viewport_keeps_all_three_lines_of_the_selected_harness_visible() {
    // One row precedes the selected three-line harness and another follows it.
    // Centering only the selected row's first line starts at zero and clips its
    // final line, while starting at one keeps the complete row in view.
    assert_eq!(super::selected_row_viewport_start(1, 4, 5, 3), 1);
}

#[test]
fn an_operator_harness_uses_one_compact_line_like_the_orchestrator() {
    let app = app();
    let lines = app.own_session_lines(&harness_row("/workspace/medulla"), false, 48, NOW);

    assert_eq!(lines.len(), 1, "a harness should consume one rail row");
    assert_eq!(
        lines[0].to_string(),
        "● codex · unmanaged · main · /workspace/medulla"
    );
}

#[test]
fn a_long_harness_path_is_shortened_instead_of_adding_rows() {
    let app = app();
    let lines = app.own_session_lines(
        &harness_row("/workspace/tinyhumans/products/medulla-public"),
        false,
        36,
        NOW,
    );

    assert_eq!(lines.len(), 1, "a long path must still use one rail row");
    assert!(lines[0].width() <= 36, "the compact row must fit the rail");
    assert!(
        lines[0].to_string().ends_with("public"),
        "the path tail should survive even when the checkout name is shortened"
    );
}

#[test]
fn a_harness_prefix_never_exceeds_the_available_width() {
    let app = app();
    for width in [0, 1, 4, 8] {
        let line = &app.own_session_lines(&harness_row("/workspace/medulla"), false, width, NOW)[0];
        assert!(line.width() <= width, "width {width}: {line:?}");
    }
}

#[test]
fn harness_branch_and_path_can_be_hidden_independently() {
    let mut app = app();
    let row = harness_row("/workspace/medulla");

    app.loaded.config.appearance.show_harness_branch = false;
    assert_eq!(
        app.own_session_lines(&row, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · /workspace/medulla"
    );

    app.loaded.config.appearance.show_harness_branch = true;
    app.loaded.config.appearance.show_harness_path = false;
    assert_eq!(
        app.own_session_lines(&row, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · main"
    );
}

#[test]
fn a_non_git_harness_omits_the_branch_without_a_placeholder() {
    let app = app();
    let mut row = harness_row("/workspace/medulla");
    row.branch = None;

    assert_eq!(
        app.own_session_lines(&row, false, 48, NOW)[0].to_string(),
        "● codex · unmanaged · /workspace/medulla"
    );
}

#[test]
fn harness_rows_color_each_lifecycle_state_and_only_the_live_ones_flash() {
    let app = app();
    let row = AgentRow::Lane { lane_index: 0 };
    let cases = [
        (Vec::new(), 0, Color::DarkGray, false, " · inactive"),
        (
            vec![task(TaskStatus::Running, false, 1)],
            1,
            Color::Green,
            true,
            " · working",
        ),
        // Blinks, like working does, and for a stronger reason: this is the one
        // state that will not resolve itself. Yellow rather than green is what
        // tells the two blinks apart.
        (
            vec![task(TaskStatus::Running, true, 1)],
            1,
            Color::Yellow,
            true,
            " · needs input",
        ),
        (
            vec![task(TaskStatus::Failed, false, 1)],
            0,
            Color::Red,
            false,
            " · errored",
        ),
        (
            vec![task(TaskStatus::Done, false, 1)],
            0,
            Color::Green,
            false,
            " · completed",
        ),
    ];

    for (tasks, active_tasks, color, flashes, suffix) in cases {
        let mut harness = lane();
        harness.tasks = tasks;
        harness.active_tasks = active_tasks;
        let line = app.agent_row_line(&row, std::slice::from_ref(&harness), false, &none_waiting());
        let style = line.spans[0].style;
        assert_eq!(style.fg, Some(color), "row: {}", line);
        assert_eq!(
            style.add_modifier.contains(Modifier::SLOW_BLINK),
            flashes,
            "row: {}",
            line
        );
        assert_eq!(app.lane_state(&harness, &none_waiting()), suffix);
    }
}

#[test]
fn current_harness_activity_takes_priority_over_an_old_error() {
    let app = app();
    let mut harness = lane();
    harness.tasks = vec![
        task(TaskStatus::Failed, false, 1),
        task(TaskStatus::Running, false, 2),
    ];
    harness.active_tasks = 1;

    let line = app.agent_row_line(
        &AgentRow::Lane { lane_index: 0 },
        std::slice::from_ref(&harness),
        false,
        &none_waiting(),
    );

    assert_eq!(line.spans[0].style.fg, Some(Color::Green));
    assert!(line.spans[0]
        .style
        .add_modifier
        .contains(Modifier::SLOW_BLINK));
    assert_eq!(app.lane_state(&harness, &none_waiting()), " · working");
}

#[test]
fn selected_working_harness_keeps_its_status_color_and_flash() {
    let app = app();
    let mut harness = lane();
    harness.tasks = vec![task(TaskStatus::Running, false, 1)];
    harness.active_tasks = 1;

    let line = app.agent_row_line(
        &AgentRow::Lane { lane_index: 0 },
        std::slice::from_ref(&harness),
        true,
        &none_waiting(),
    );
    let style = line.spans[0].style;

    assert_eq!(style.fg, Some(Color::Green));
    assert_eq!(style.bg, Some(app.theme.primary));
    assert!(style.add_modifier.contains(Modifier::SLOW_BLINK));
}
