//! Unit tests for slash-command parsing and the `/copy` helper.

use super::*;
use crate::ui::events::{EventEnvelope, TuiEvent};

fn ev(seq: u64, event: TuiEvent) -> EventEnvelope {
    EventEnvelope { seq, at: 0, event }
}

#[test]
fn non_slash_input_is_not_a_command() {
    assert_eq!(parse("hello world"), None);
    assert_eq!(parse("   "), None);
}

#[test]
fn simple_commands_and_aliases() {
    assert_eq!(parse("/quit"), Some(SlashCommand::Quit));
    assert_eq!(parse("/q"), Some(SlashCommand::Quit));
    assert_eq!(parse("/new"), Some(SlashCommand::NewSession));
    assert_eq!(parse("/resume"), Some(SlashCommand::Resume));
    assert_eq!(parse("/abort"), Some(SlashCommand::Abort));
    assert_eq!(parse("/clear"), Some(SlashCommand::ClearView));
    assert_eq!(parse("/settings"), Some(SlashCommand::Settings));
    assert_eq!(parse("/theme"), Some(SlashCommand::Settings));
    assert_eq!(parse("/mem"), Some(SlashCommand::Memory(None)));
}

#[test]
fn command_token_is_case_insensitive() {
    assert_eq!(parse("/QUIT"), Some(SlashCommand::Quit));
    assert_eq!(parse("  /New  "), Some(SlashCommand::NewSession));
}

#[test]
fn fork_and_memory_preserve_argument_case() {
    assert_eq!(
        parse("/fork My Feature"),
        Some(SlashCommand::Fork(Some("My Feature".into())))
    );
    assert_eq!(parse("/fork"), Some(SlashCommand::Fork(None)));
    assert_eq!(
        parse("/memory Find The Thing"),
        Some(SlashCommand::Memory(Some("Find The Thing".into())))
    );
}

#[test]
fn lesson_parses_the_shared_trigger_rule_shape() {
    assert_eq!(
        parse("/lesson CI flakes -> rerun the focused test"),
        Some(SlashCommand::Lesson {
            trigger: "CI flakes".into(),
            rule: "rerun the focused test".into(),
        })
    );
    assert_eq!(
        parse("/lesson missing delimiter"),
        Some(SlashCommand::BadUsage("Usage: /lesson <trigger> -> <rule>"))
    );
}

#[test]
fn review_requires_and_preserves_a_target() {
    assert_eq!(
        parse("/review Task-42"),
        Some(SlashCommand::Review("Task-42".into()))
    );
    assert_eq!(
        parse("/review"),
        Some(SlashCommand::BadUsage("Usage: /review <lane|task-id>"))
    );
}

#[test]
fn copy_validates_scope() {
    assert_eq!(parse("/copy"), Some(SlashCommand::Copy(CopyScope::All)));
    assert_eq!(parse("/copy all"), Some(SlashCommand::Copy(CopyScope::All)));
    assert_eq!(
        parse("/copy LAST"),
        Some(SlashCommand::Copy(CopyScope::Last))
    );
    assert_eq!(
        parse("/copy bogus"),
        Some(SlashCommand::BadUsage("Usage: /copy [all|last]"))
    );
}

#[test]
fn async_toggles_and_validates() {
    assert_eq!(parse("/async"), Some(SlashCommand::Async(None)));
    assert_eq!(parse("/async on"), Some(SlashCommand::Async(Some(true))));
    assert_eq!(parse("/async OFF"), Some(SlashCommand::Async(Some(false))));
    assert_eq!(
        parse("/async maybe"),
        Some(SlashCommand::BadUsage("Usage: /async [on|off]"))
    );
}

#[test]
fn unknown_command_carries_original_input() {
    assert_eq!(
        parse("/frobnicate x"),
        Some(SlashCommand::Unknown("/frobnicate x".into()))
    );
}

#[test]
fn copy_text_scopes() {
    let events = vec![
        ev(1, TuiEvent::User { body: "q".into() }),
        ev(2, TuiEvent::Assistant { body: "a".into() }),
    ];
    assert_eq!(copy_text(&events, CopyScope::Last), "a");
    assert_eq!(copy_text(&events, CopyScope::All), "> q\n\na");
}

#[test]
fn copy_text_last_is_empty_without_assistant() {
    let events = vec![ev(1, TuiEvent::User { body: "q".into() })];
    assert_eq!(copy_text(&events, CopyScope::Last), "");
    assert_eq!(copy_text(&[], CopyScope::All), "");
}
