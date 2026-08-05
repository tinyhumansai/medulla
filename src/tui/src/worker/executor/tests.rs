//! Focused tests for PTY executor lifecycle decisions.

use medulla::sessions::SessionClass;

use std::collections::HashMap;

use super::hold::handback_prompt;
use super::run::{retains_workspace_context, retire_stopped_workspace_context};
use crate::worker::pty::SessionControl;

#[test]
fn mapper_context_survives_only_for_live_reusable_sessions() {
    assert!(retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::User),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        false,
    ));
}

#[test]
fn a_failed_orchestrator_stop_keeps_operator_owned_mapper_context() {
    let mut context = HashMap::from([(
        "pty-1".to_string(),
        (
            Some("/repo/worktrees/fix".to_string()),
            Some("fix".to_string()),
            Some("https://github.com/acme/repo/pull/42".to_string()),
        ),
    )]);
    retire_stopped_workspace_context(&mut context, "pty-1", false);
    assert!(context.contains_key("pty-1"));

    retire_stopped_workspace_context(&mut context, "pty-1", true);
    assert!(!context.contains_key("pty-1"));
}

#[test]
fn a_multi_line_instruction_still_yields_a_one_line_prompt_with_its_tail() {
    // The prompt is typed into a composer, so a line-oriented harness reads it
    // up to the first newline. An instruction carrying one — a bulleted brief, a
    // pasted trace — used to submit everything before it and silently drop the
    // rest of the sentence, which is where the do-not-redo guidance lives: the
    // one thing the hand-back turn exists to say.
    let prompt = handback_prompt("fix the parser\n- keep the tests green\n\n- then report");

    assert!(
        !prompt.contains('\n'),
        "the prompt is one line or it is truncated: {prompt:?}"
    );
    assert!(
        prompt.contains("fix the parser - keep the tests green - then report"),
        "the whole instruction survives, flattened: {prompt:?}"
    );
    assert!(
        prompt.contains("do not redo it"),
        "and so does the tail after it: {prompt:?}"
    );
    assert!(prompt.contains("complete it."), "to the end: {prompt:?}");

    // A single-line instruction is passed through unchanged.
    assert!(handback_prompt("fix the parser").contains("this task: fix the parser — if the"));
}
