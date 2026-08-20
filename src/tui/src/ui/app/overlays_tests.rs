//! That the overlay list stays the single answer to "what is in front of the
//! content pane?".
//!
//! The render iterates it and `overlay_owns_keys` is derived from it, so the
//! thing worth pinning here is the list itself: every variant must be reachable,
//! and each must make `overlay_owns_keys` true on its own. A variant that no
//! condition can produce would be an overlay the render never paints; one that
//! left `overlay_owns_keys` false would be an overlay drawn over a composer that
//! kept taking input behind it, which is the bug this module exists to prevent.
//!
//! The paste side of it — that each overlay actually stops a payload reaching
//! the composer underneath — is covered end to end in `tests/feature_paste.rs`.

use std::sync::Arc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;

use super::types::{
    tab_pos, App, HandbackPrompt, Overlay, PromptKind, ResumePicker, SessionPicker,
    SessionPickerStep, WorkspaceChoice, RP_TEMPLATES,
};
use crate::ui::composer::{Draft, TextPrompt};

fn app() -> App {
    App::new(
        Arc::new(MockRuntime::empty()),
        LoadedConfig::defaults("medulla.tui.json".into()),
    )
}

/// Put `overlay` on screen by the shortest honest route.
fn raise(app: &mut App, overlay: Overlay) {
    match overlay {
        Overlay::Decisions => app.decision_open = true,
        Overlay::TemplatePopup => {
            app.template_modal = true;
            app.tab_index = tab_pos("Hosts");
            app.routing_index = RP_TEMPLATES;
        }
        Overlay::SessionPicker => {
            app.session_picker = Some(SessionPicker {
                choices: Vec::new(),
                index: 0,
                step: SessionPickerStep::Harness,
                cwd: "/".into(),
                workspace_query: String::new(),
                workspace_choices: Vec::new(),
                workspace_index: 0,
                workspace_picked: false,
            })
        }
        Overlay::SessionKill => app.arm_harness_close("s".into()),
        Overlay::HandbackPrompt => {
            app.handback_prompt = Some(HandbackPrompt {
                session: "s".into(),
                took_control: false,
                note: Draft::default(),
                editing_note: false,
                is_takeover: false,
            })
        }
        Overlay::WorkflowDelete => app.workflow_delete_armed = Some(("id".into(), "name".into())),
        Overlay::InlinePrompt => {
            app.prompt = Some(TextPrompt::new(PromptKind::HostAdd, "Add a host"))
        }
        Overlay::ResumePicker => {
            app.resume_picker = Some(ResumePicker {
                chats: Vec::new(),
                index: 0,
            })
        }
    }
}

/// Every variant, so a new one cannot be added without appearing here.
const EVERY_OVERLAY: [Overlay; 8] = [
    Overlay::Decisions,
    Overlay::TemplatePopup,
    Overlay::SessionPicker,
    Overlay::SessionKill,
    Overlay::HandbackPrompt,
    Overlay::WorkflowDelete,
    Overlay::InlinePrompt,
    Overlay::ResumePicker,
];

#[test]
fn nothing_is_in_front_of_a_freshly_opened_screen() {
    let app = app();

    assert!(app.visible_overlays().is_empty());
    assert!(!app.overlay_owns_keys());
}

#[test]
fn every_overlay_is_reachable_and_owns_input_on_its_own() {
    // The load-bearing property: the render paints from this list, so a variant
    // that no state can produce is dead paint, and one that leaves
    // `overlay_owns_keys` false is an overlay covering a composer that goes on
    // accepting keystrokes and pastes nobody can see arriving.
    for overlay in EVERY_OVERLAY {
        let mut app = app();
        raise(&mut app, overlay);

        assert_eq!(
            app.visible_overlays(),
            vec![overlay],
            "{overlay:?} should be the one thing on screen"
        );
        assert!(
            app.overlay_owns_keys(),
            "{overlay:?} is drawn over the content, so it owns input"
        );
    }
}

#[test]
fn handback_prompt_swallows_clicks_behind_it() {
    let mut app = app();
    raise(&mut app, Overlay::HandbackPrompt);
    app.hit_tabs_row = 1;
    app.hit_tabs = vec![(0, 4), (5, 10)];
    let original_tab = app.tab_index;

    let _ = app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(
        app.tab_index, original_tab,
        "a handback prompt must not let a click navigate the UI behind it"
    );
    assert!(
        app.handback_prompt.is_some(),
        "the click must leave the pending harness decision intact"
    );
}

#[test]
fn inline_prompt_swallows_clicks_that_would_reach_the_picker_behind_it() {
    // The favorite-add prompt opens *on top of* the workspace picker (Shift+F).
    // Keyboard routing gives the prompt precedence over the picker, and the
    // pointer must not come to disagree: a click on a picker row behind the
    // prompt would replay Enter and start a harness while the favorite-name
    // edit is still on screen.
    let mut app = app();
    app.session_picker = Some(SessionPicker {
        choices: Vec::new(),
        index: 0,
        step: SessionPickerStep::Workspace,
        cwd: "/".into(),
        workspace_query: "x".into(),
        workspace_choices: vec![WorkspaceChoice {
            path: "/tmp".into(),
            source: "folder".into(),
            label: None,
        }],
        workspace_index: 0,
        workspace_picked: false,
    });
    app.prompt = Some(TextPrompt::new(
        PromptKind::FavoriteWorkspaceAdd("/tmp".into()),
        "Save favorite for /tmp",
    ));
    app.hit_session_picker = Some((
        ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 18,
        },
        vec![(
            ratatui::layout::Rect {
                x: 0,
                y: 5,
                width: 60,
                height: 1,
            },
            0,
        )],
    ));

    let _ = app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        app.prompt.is_some(),
        "the click must not dismiss the favorite prompt"
    );
    let picker = app.session_picker.as_ref().unwrap();
    assert!(
        !picker.workspace_picked,
        "the click must not be read as choosing a workspace row"
    );
    assert_eq!(
        picker.workspace_index, 0,
        "the picker selection must be left where it was"
    );
}

#[test]
fn pointer_input_cancels_an_armed_harness_close() {
    let mut app = app();
    app.arm_harness_close("session-a".into());

    let _ = app.on_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.harness_close_armed.is_none());
    assert!(app.status().contains("cancelled"), "{}", app.status());
}

#[test]
fn cancelling_a_session_kill_with_the_mouse_does_not_click_through_the_modal() {
    let mut app = app();
    app.arm_harness_close("session-a".into());
    app.hit_tabs_row = 1;
    app.hit_tabs = vec![(0, 4), (5, 10)];
    let original_tab = app.tab_index;

    let _ = app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.harness_close_armed.is_none());
    assert_eq!(
        app.tab_index, original_tab,
        "the cancellation click is consumed"
    );
}

#[test]
fn pointer_input_cancels_workflow_delete_without_clicking_through() {
    let mut app = app();
    app.workflow_delete_armed = Some(("nightly".into(), "Nightly sweep".into()));
    app.hit_tabs_row = 1;
    app.hit_tabs = vec![(0, 4), (5, 10)];
    let original_tab = app.tab_index;

    let result = app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });

    assert!(result.is_none());
    assert!(app.workflow_delete_armed.is_none());
    assert_eq!(app.status(), "Workflow deletion cancelled");
    assert_eq!(app.tab_index, original_tab, "the cancellation click is consumed");
}

#[test]
fn no_overlay_lets_a_paste_through_to_the_composer_behind_it() {
    // The whole point of deriving `overlay_owns_keys` from this list. Asserted
    // per overlay rather than once, because the failure mode is always a single
    // overlay that nobody added to the condition: the resume picker, then the
    // template popup, each of which sat over a composer that went on quietly
    // banking pastes to be submitted when it closed.
    //
    // The template popup is on Hosts by construction — it cannot coexist with
    // the Sessions composer, which is the fix — so the tab is set first and only
    // where the overlay permits it.
    for overlay in EVERY_OVERLAY {
        let mut app = app();
        app.tab_index = tab_pos("Sessions");
        raise(&mut app, overlay);

        app.on_event(crossterm::event::Event::Paste("stray text".into()));

        assert_eq!(
            app.draft_text(),
            "",
            "{overlay:?} is in front of the composer, so the paste is not its text"
        );
    }
}

#[test]
fn the_template_popup_needs_the_page_that_can_dismiss_it() {
    // `Tab` switches tabs with the popup open and nothing clears the flag, so
    // the flag alone is not "the popup is up". Reading it as such drew the popup
    // over another tab, where none of its keys are bound.
    let mut app = app();
    raise(&mut app, Overlay::TemplatePopup);
    assert_eq!(app.visible_overlays(), vec![Overlay::TemplatePopup]);

    app.tab_index = tab_pos("Sessions");

    assert!(
        app.visible_overlays().is_empty(),
        "the popup does not follow the operator off its own page"
    );
    assert!(!app.overlay_owns_keys());
}

#[test]
fn the_inline_prompt_replaces_the_resume_picker_rather_than_stacking_on_it() {
    // They share the row below the content and the prompt wins, so listing both
    // would claim something is on screen that the render never draws.
    let mut app = app();
    raise(&mut app, Overlay::ResumePicker);
    raise(&mut app, Overlay::InlinePrompt);

    assert_eq!(app.visible_overlays(), vec![Overlay::InlinePrompt]);
}

#[test]
fn overlays_are_listed_back_to_front_in_the_order_the_render_paints_them() {
    // The list is iterated to paint, so its order is the stacking order: the
    // hand-back question is asked over the picker that may have opened it.
    let mut app = app();
    raise(&mut app, Overlay::SessionPicker);
    raise(&mut app, Overlay::HandbackPrompt);
    raise(&mut app, Overlay::Decisions);

    assert_eq!(
        app.visible_overlays(),
        vec![
            Overlay::Decisions,
            Overlay::SessionPicker,
            Overlay::HandbackPrompt
        ]
    );
}

#[test]
fn a_handback_note_wider_than_the_modal_wraps_instead_of_vanishing() {
    // Reported: typing a note into the leave-a-session prompt, everything past
    // the visible width disappeared. The keystrokes still landed — the draft
    // grew — but the operator could not read back or edit what they wrote,
    // which is the one moment they actually have the context to write it.
    //
    // The note is one `TLine` holding the whole draft, so nothing folds it
    // unless the `Paragraph` is told to wrap; ratatui clips an over-long line
    // by default. The modal also grows a row per wrapped line, or wrapping just
    // pushes the answer hint out of a fixed 12-row box instead.
    let mut app = app();
    raise(&mut app, Overlay::HandbackPrompt);
    let note = "continue the migration: the socketioxide bump is done, next is \
                the whisper README pass, then rerun the e2e suite";
    if let Some(prompt) = app.handback_prompt.as_mut() {
        prompt.editing_note = true;
        prompt.note.text = note.to_string();
    }

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(
        screen.contains("continue the migration"),
        "the start of the note should be visible"
    );
    // The tail lands on a later row rather than being clipped away.
    assert!(
        screen.contains("rerun the e2e suite"),
        "the end of a wrapped note should be on screen"
    );
    // And the hint the wrapped rows could have displaced is still drawn.
    assert!(
        screen.contains("Type your note"),
        "wrapping must grow the modal, not push its answer hint off the bottom"
    );
}

#[test]
fn a_click_answers_the_question_even_after_the_note_has_wrapped() {
    // The wrapped note pushes the answer hint down a row. The hit boxes are
    // recorded during the draw, so they have to follow it — a click aimed at
    // the unwrapped row would land on the note's continuation and answer
    // nothing, which is worse than the clipping this replaced: the controls are
    // visible and simply do not respond.
    //
    // So the click is aimed where "[Y] hand back" is actually PAINTED, found by
    // reading the rendered buffer back. Clicking the recorded box instead would
    // pass whether or not the box agrees with the screen, which is the only
    // thing worth testing here.
    let mut app = app();
    raise(&mut app, Overlay::HandbackPrompt);
    if let Some(prompt) = app.handback_prompt.as_mut() {
        prompt.note.text = "x".repeat(180);
    }

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");

    let buffer = terminal.backend().buffer().clone();
    let painted = (0..buffer.area.height)
        .find_map(|y| {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.find("[Y] hand back").map(|x| (x as u16, y))
        })
        .expect("the answer hint should be painted somewhere");

    let _ = app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: painted.0,
        row: painted.1,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        app.handback_prompt.is_none(),
        "clicking the visible answer must resolve the question once the note wraps"
    );
}

#[test]
fn a_modal_too_tall_for_the_terminal_records_no_offscreen_answers() {
    // `centered` clamps to the terminal, so a long note on a short screen
    // clips the hint instead of drawing it. Recording boxes for a row that was
    // never painted would answer the question for a click that landed on the
    // pane behind the modal.
    let mut app = app();
    raise(&mut app, Overlay::HandbackPrompt);
    if let Some(prompt) = app.handback_prompt.as_mut() {
        prompt.note.text = "y".repeat(600);
    }

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 10)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");

    for (rect, _) in &app.hit_handback {
        assert!(
            rect.y < 10,
            "a hit box must never sit below the terminal it was drawn in"
        );
    }
}
