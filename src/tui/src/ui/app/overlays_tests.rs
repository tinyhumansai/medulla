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
    tab_pos, App, HandbackPrompt, HarnessPicker, HarnessPickerStep, Overlay, PromptKind,
    ResumePicker, RP_TEMPLATES,
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
        Overlay::HarnessPicker => {
            app.harness_picker = Some(HarnessPicker {
                purpose: super::types::PickerPurpose::Spawn,
                choices: Vec::new(),
                index: 0,
                step: HarnessPickerStep::Harness,
                cwd: "/".into(),
                workspace_query: String::new(),
                workspace_choices: Vec::new(),
                workspace_index: 0,
                workspace_picked: false,
                managed: true,
            })
        }
        Overlay::HandbackPrompt => {
            app.handback_prompt = Some(HandbackPrompt {
                session: "s".into(),
                took_control: false,
                note: Draft::default(),
                editing_note: false,
                is_takeover: false,
            })
        }
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
const EVERY_OVERLAY: [Overlay; 6] = [
    Overlay::Decisions,
    Overlay::TemplatePopup,
    Overlay::HarnessPicker,
    Overlay::HandbackPrompt,
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
fn no_overlay_lets_a_paste_through_to_the_composer_behind_it() {
    // The whole point of deriving `overlay_owns_keys` from this list. Asserted
    // per overlay rather than once, because the failure mode is always a single
    // overlay that nobody added to the condition: the resume picker, then the
    // template popup, each of which sat over a composer that went on quietly
    // banking pastes to be submitted when it closed.
    //
    // The template popup is on Hosts by construction — it cannot coexist with
    // the Agents composer, which is the fix — so the tab is set first and only
    // where the overlay permits it.
    for overlay in EVERY_OVERLAY {
        let mut app = app();
        app.tab_index = tab_pos("Agents");
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

    app.tab_index = tab_pos("Agents");

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
    raise(&mut app, Overlay::HarnessPicker);
    raise(&mut app, Overlay::HandbackPrompt);
    raise(&mut app, Overlay::Decisions);

    assert_eq!(
        app.visible_overlays(),
        vec![
            Overlay::Decisions,
            Overlay::HarnessPicker,
            Overlay::HandbackPrompt
        ]
    );
}
