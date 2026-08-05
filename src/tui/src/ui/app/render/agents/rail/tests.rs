//! Tests for the Agents rail's line layout: the width cap, how a row that does
//! not fit is re-flowed, and what a working directory keeps when it cannot.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::agents::{AgentLane, AgentRole, AgentRow, TaskState, TaskStatus};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use crate::ui::app::App;
use crate::worker::pty::{AttentionKind, HarnessAttention, PtyState, SessionControl, SessionRow};

use super::rows::{display_session_title, running_session_title};
use super::wrap::{flow_path, short_home, wrap_line, wrap_path};

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
        name: None,
        attention: None,
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

#[test]
fn a_row_that_fits_is_left_exactly_as_it_was() {
    let line = TLine::from(Span::raw("● orchestrator · 1"));
    let out = wrap_line(&line, 36, 5);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].width(), line.width());
}

#[test]
fn an_overlong_row_wraps_and_its_continuation_is_indented() {
    let line = TLine::from(Span::raw("● [CODEX] dev-1 · 3 · ctx 6.4k · 1/1 sess · 2/4"));
    let out = wrap_line(&line, 24, 5);

    assert!(out.len() > 1, "the row does not fit on one line");
    for wrapped in &out {
        assert!(
            wrapped.width() <= 24,
            "no line may overrun the pane: {wrapped:?}"
        );
    }
    let second: String = out[1].spans.iter().map(|s| s.content.to_string()).collect();
    assert!(
        second.starts_with("     "),
        "a continuation line is indented so the row still reads as one: {second:?}"
    );
}

#[test]
fn wrapping_keeps_each_span_style() {
    // A task row colours its status word by status. Re-flowing through a plain
    // string would hand the whole row one style and lose that entirely.
    let coloured = Style::default().add_modifier(Modifier::BOLD);
    let line = TLine::from(vec![
        Span::raw("   task-1 aaaa bbbb cccc "),
        Span::styled("running", coloured),
        Span::raw(" · 9 turns"),
    ]);
    let out = wrap_line(&line, 20, 5);

    let kept: usize = out
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|span| span.style == coloured)
        .map(|span| span.content.chars().count())
        .sum();
    assert_eq!(kept, "running".len(), "the styled run survives the wrap");
}

#[test]
fn wide_characters_wrap_by_display_column_not_char_count() {
    // Each CJK character below occupies two terminal columns. A char-count
    // wrap would accept twice as many as actually fit and clip the row
    // instead of wrapping it.
    let line = TLine::from(Span::raw("任务一二三四五六七八九十"));
    let out = wrap_line(&line, 10, 0);

    assert!(out.len() > 1, "a row this wide must wrap: {out:?}");
    for wrapped in &out {
        assert!(
            wrapped.width() <= 10,
            "no line may overrun the pane: {wrapped:?}"
        );
    }
}

#[test]
fn a_path_breaks_on_separators() {
    let out = flow_path("~/work/tinyhumans/medulla", 12);
    for line in &out {
        assert!(line.chars().count() <= 12, "{line:?} overruns");
    }
    assert_eq!(out.concat(), "~/work/tinyhumans/medulla");
}

#[test]
fn a_path_too_long_to_fit_keeps_its_tail() {
    // The tail names the checkout; the head is what every sibling harness on
    // the machine shares, so dropping it loses nothing and dropping the tail
    // loses the only fact the row was drawn for.
    let out = wrap_path(
        "~/work/some-org/some-umbrella/worktrees/agents-ux/medulla-public",
        28,
        2,
    );

    assert!(
        out.len() <= 2,
        "the path is held to its line budget: {out:?}"
    );
    let joined = out.concat();
    assert!(
        joined.ends_with("medulla-public"),
        "the end of the path survives: {joined:?}"
    );
    assert!(
        joined.starts_with('…'),
        "and the row says it was shortened: {joined:?}"
    );
}

#[test]
fn a_path_segment_of_wide_characters_hard_cuts_by_display_column() {
    // A single unbreakable segment made of two-column characters must still
    // land within the width budget per line, not the char-count budget.
    let out = flow_path("任务一二三四五六七八九十", 10);
    for line in &out {
        assert!(line.width() <= 10, "{line:?} overruns");
    }
    assert_eq!(out.concat(), "任务一二三四五六七八九十");
}

#[test]
fn an_unbreakable_final_segment_keeps_its_own_tail() {
    // When the checkout name itself (the part after the last `/`) is longer
    // than the whole line budget, there is no separator left to drop a head
    // at. The name still has to keep its distinguishing suffix rather than
    // silently losing it to a head-first hard cut, or two harnesses that
    // share a long prefix would render identically.
    let out = wrap_path(
        "~/work/a/very-long-checkout-name-that-alone-overruns-the-budget-abcdefg",
        12,
        2,
    );

    assert!(
        out.len() <= 2,
        "the path is held to its line budget: {out:?}"
    );
    let joined = out.concat();
    assert!(
        joined.ends_with("abcdefg"),
        "the tail of the unbreakable segment survives: {joined:?}"
    );
    assert!(
        joined.starts_with('…'),
        "and the row says it was shortened: {joined:?}"
    );
}

#[test]
fn the_home_directory_collapses_to_a_tilde() {
    // A fixture, not the real environment. Reading `$HOME` here panicked the
    // whole suite on Windows, which sets `USERPROFILE` and no `HOME` — and a
    // presentation rule should not depend on the machine running it anyway.
    let home = Some("/Users/dev");
    assert_eq!(short_home("/Users/dev/work/repo", home), "~/work/repo");
    assert_eq!(short_home("/Users/dev", home), "~");
    assert_eq!(short_home("/Users/dev/", home), "~");
    // A path outside home keeps its head; there is nothing to collapse.
    assert_eq!(short_home("/srv/repos/auth", home), "/srv/repos/auth");
    // A sibling that merely *starts* with the same characters is not inside
    // home, so it must not be rewritten as though it were.
    assert_eq!(short_home("/Users/developer/x", home), "/Users/developer/x");
}

#[test]
fn a_windows_home_collapses_on_its_own_separator() {
    // Windows hands us `C:\Users\dev` while the harness reports whichever
    // separator it was started with, so both have to be recognised.
    let home = Some("C:\\Users\\dev");
    assert_eq!(
        short_home("C:\\Users\\dev\\work\\repo", home),
        "~\\work\\repo"
    );
    assert_eq!(short_home("C:\\Users\\dev", home), "~");
    assert_eq!(short_home("D:\\src\\other", home), "D:\\src\\other");
}

#[test]
fn an_unknown_home_leaves_the_path_alone() {
    // `dirs::home_dir()` can fail. Showing the absolute path is right then —
    // inventing a `~` for a directory we cannot place would be a lie.
    assert_eq!(short_home("/srv/repos/auth", None), "/srv/repos/auth");
    assert_eq!(short_home("/srv/repos/auth", Some("")), "/srv/repos/auth");
}
