//! Agents-tab coverage: subtask lane rendering and the `More` overflow, X-cancel
//! and A-answer steering, the inline answer-prompt overlay editing lifecycle,
//! j/k scrolling, and cancel of a cycle-less task id.

use crate::helpers::*;

// --- Agents lane rendering: Sub rows, the More overflow, task transcript -----

#[test]
fn agents_renders_subtask_rows_more_overflow_and_task_transcript() {
    let (mut app, rt) = demo_app();
    // Delegate ten tasks to the rostered dev-1 agent so its lane overflows the
    // 8-subtask cap (→ a `More` row) and shows individual `Sub` rows.
    for n in 0..10 {
        rt.script_event(TuiEvent::TaskStart {
            task_id: format!("cyc-1/t:job{n}"),
            instruction: format!("job number {n}"),
            depth: 2,
            agent_id: Some("dev-1".into()),
        });
    }
    // One completes with usage, which lights the lane's context-token bar.
    rt.script_event(TuiEvent::TaskComplete {
        digest: TaskDigest {
            task_id: "cyc-1/t:job0".into(),
            status: "done".into(),
            digest: "finished job 0".into(),
            result_ref: None,
            usage: Some(Usage {
                input_tokens: 5000,
                output_tokens: 300,
            }),
            depth: 2,
        },
    });
    app.refresh_snapshot();
    tab(&mut app, "Agents");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("more"), "the +N more overflow row renders");
    assert!(out.contains("job"), "sub-task rows render");

    // Drive the cursor onto a task row and re-render to exercise the task
    // transcript pane (and its context bar) rather than the lane transcript.
    for _ in 0..20 {
        if app.selected_task_id().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    assert!(app.selected_task_id().is_some(), "landed on a Sub row");
    let out = render(&mut app, 120, 40);
    assert!(out.contains("turns"), "task transcript header renders");
}

#[test]
fn agents_x_on_a_lane_row_prompts_to_select_a_task() {
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    // Default cursor sits on a tier lane, not a task row.
    assert!(app.selected_task_id().is_none());
    let _ = app.on_event(key(KeyCode::Char('X')));
    assert!(
        app.status().contains("Select a running task"),
        "status: {}",
        app.status()
    );
}

#[test]
fn agents_x_cancels_selected_cycle_task() {
    let (mut app, _rt) = app_with_selected_task();
    assert_eq!(app.selected_task_id().as_deref(), Some("cyc-9/t:q1"));
    let _ = app.on_event(key(KeyCode::Char('X')));
    // The bare task id (after the cycle prefix) appears in the confirmation.
    assert!(
        app.status().contains("Cancel requested") && app.status().contains("q1"),
        "status: {}",
        app.status()
    );
}

#[test]
fn agents_a_opens_the_answer_prompt() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let (title, draft) = app.prompt_state().expect("answer prompt should open");
    assert!(title.starts_with("Answer"), "title: {title}");
    assert!(draft.is_empty());
    // Rendering the overlay shows the magenta prompt caret.
    let out = render(&mut app, 120, 40);
    assert!(out.contains("Answer"), "prompt title should render");
}

#[test]
fn agents_a_on_task_without_question_reports_none() {
    // The demo's task-1 is complete with no pending question.
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    for _ in 0..12 {
        if app.selected_task_id().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    // Only proceed if we actually landed on the (question-less) task row.
    if app.selected_task_id().is_some() {
        let _ = app.on_event(key(KeyCode::Char('A')));
        assert!(
            app.status().contains("no pending question") || app.prompt_state().is_none(),
            "status: {}",
            app.status()
        );
    }
}

// --- inline prompt overlay editing ------------------------------------------

#[test]
fn prompt_answer_typing_editing_and_send() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    assert!(app.prompt_state().is_some());
    type_str(&mut app, "yess");
    // Backspace trims the stray char.
    let _ = app.on_event(key(KeyCode::Backspace));
    assert_eq!(app.prompt_state().unwrap().1, "yes");
    // Left then insert in the middle.
    let _ = app.on_event(key(KeyCode::Left));
    type_str(&mut app, "X");
    assert_eq!(app.prompt_state().unwrap().1, "yeXs");
    // Right + Enter sends and closes the overlay (answer_question is a no-op here).
    let _ = app.on_event(key(KeyCode::Right));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(cmd.is_none());
    assert!(app.prompt_state().is_none());
    assert!(
        app.status().contains("Answer sent"),
        "status: {}",
        app.status()
    );
}

#[test]
fn prompt_esc_cancels_and_ctrl_c_quits() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let _ = app.on_event(key(KeyCode::Esc));
    assert!(app.prompt_state().is_none());
    assert!(app.status().contains("Cancelled"));

    let _ = app.on_event(key(KeyCode::Char('A')));
    assert!(app.prompt_state().is_some());
    let _ = app.on_event(ctrl(KeyCode::Char('c')));
    assert!(app.should_quit);
}

#[test]
fn prompt_empty_answer_is_cancelled() {
    let (mut app, _rt) = app_with_selected_task();
    let _ = app.on_event(key(KeyCode::Char('A')));
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(cmd.is_none());
    assert!(
        app.status().contains("Answer cancelled"),
        "status: {}",
        app.status()
    );
}

// --- Agents j/k scroll & agent-index navigation -----------------------------

#[test]
fn agents_jk_scroll_and_arrow_nav() {
    let (mut app, _rt) = demo_app();
    tab(&mut app, "Agents");
    let _ = render(&mut app, 120, 40);
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('j')));
    let _ = app.on_event(key(KeyCode::Char('k')));
    // Arrow up/down move the agent cursor across selectable rows without panic.
    for _ in 0..15 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    for _ in 0..15 {
        let _ = app.on_event(key(KeyCode::Up));
    }
    let _ = render(&mut app, 120, 40);
}

// --- cancel with a cycle-less task id ---------------------------------------

#[test]
fn cancel_task_without_cycle_prefix_reports_no_cycle() {
    let (mut app, rt) = demo_app();
    // A bare task id (no `/t:` cycle prefix) yields a Sub row with no cycle.
    rt.script_event(TuiEvent::TaskStart {
        task_id: "bare-task".into(),
        instruction: "go".into(),
        depth: 2,
        agent_id: Some("dev-1".into()),
    });
    app.refresh_snapshot();
    tab(&mut app, "Agents");
    for _ in 0..14 {
        if app.selected_task_id().as_deref() == Some("bare-task") {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
    }
    if app.selected_task_id().as_deref() == Some("bare-task") {
        let _ = app.on_event(key(KeyCode::Char('X')));
        assert!(
            app.status().contains("no cycle"),
            "status: {}",
            app.status()
        );
    }
}
