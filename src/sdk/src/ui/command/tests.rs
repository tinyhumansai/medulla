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
    assert_eq!(parse("/fb"), Some(SlashCommand::Feedback));
}

#[test]
fn command_token_is_case_insensitive() {
    assert_eq!(parse("/QUIT"), Some(SlashCommand::Quit));
    assert_eq!(parse("  /New  "), Some(SlashCommand::NewSession));
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

// --- the catalog ------------------------------------------------------------

#[test]
fn every_catalogued_command_parses_to_something_real() {
    // The catalog is what the peek offers and what Help lists, so an entry that
    // does not parse is a command the operator can pick and watch fail.
    for spec in COMMANDS {
        for name in std::iter::once(spec.name).chain(spec.aliases.iter().copied()) {
            let parsed = parse(&format!("/{name}"));
            assert!(
                !matches!(parsed, None | Some(SlashCommand::Unknown(_))),
                "/{name} is offered but does not parse: {parsed:?}"
            );
        }
    }
}

#[test]
fn every_parseable_command_is_catalogued() {
    // The other direction: a command the parser knows but the catalog omits is
    // undiscoverable — it exists only for whoever read the source.
    for name in [
        "new", "resume", "abort", "clear", "copy", "usage", "settings", "theme", "config",
        "feedback", "fb", "mouse", "help", "quit", "exit", "q",
    ] {
        assert!(
            lookup(name).is_some(),
            "/{name} parses but is not catalogued"
        );
    }
}

#[test]
fn exit_is_a_spelling_of_quit() {
    assert_eq!(parse("/exit"), Some(SlashCommand::Quit));
    assert_eq!(parse("/q"), Some(SlashCommand::Quit));
    assert_eq!(parse("/QUIT"), Some(SlashCommand::Quit));
}

#[test]
fn suggestions_narrow_as_the_name_is_typed() {
    // A bare slash offers everything…
    let all = suggestions("/").expect("a slash opens the peek");
    assert_eq!(all.len(), COMMANDS.len());

    // …a prefix narrows it, by alias as well as by name…
    let names: Vec<&str> = suggestions("/fb")
        .expect("still choosing")
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(names, vec!["feedback"], "matched via its `fb` alias");

    // …an unknown prefix says so with an empty list rather than by vanishing…
    assert_eq!(suggestions("/zzz").expect("still a command line").len(), 0);

    // …and once an argument is being typed the choice is made.
    assert!(suggestions("/copy last").is_none());
    assert!(suggestions("hello").is_none(), "not a command at all");
}

#[test]
fn usage_renders_the_argument_hint_only_when_there_is_one() {
    assert_eq!(lookup("copy").unwrap().usage(), "/copy [all|last]");
    assert_eq!(lookup("clear").unwrap().usage(), "/clear");
}

#[test]
fn harness_takes_an_optional_provider_and_path() {
    // Bare: the front end opens its picker rather than guessing.
    assert_eq!(
        parse("/harness"),
        Some(SlashCommand::NewHarness {
            provider: None,
            path: None,
        })
    );
    assert_eq!(
        parse("/harness codex"),
        Some(SlashCommand::NewHarness {
            provider: Some("codex".to_string()),
            path: None,
        })
    );
    assert_eq!(
        parse("/harness Claude ~/work/foo"),
        Some(SlashCommand::NewHarness {
            provider: Some("claude".to_string()),
            path: Some("~/work/foo".to_string()),
        }),
        "the provider is matched case-insensitively, the path is left alone"
    );
}

#[test]
fn an_unknown_harness_provider_is_a_usage_error_not_a_default() {
    // Silently falling back to the default provider would start the wrong CLI
    // in the operator's workspace, and they would not find out until it did
    // something.
    assert_eq!(
        parse("/harness claud"),
        Some(SlashCommand::BadUsage(
            "Usage: /harness [claude|codex|opencode] [path]"
        ))
    );
    assert_eq!(
        parse("/harness ~/work/foo"),
        Some(SlashCommand::BadUsage(
            "Usage: /harness [claude|codex|opencode] [path]"
        )),
        "a bare path is ambiguous with a provider name, so it is refused"
    );
}

#[test]
fn control_handover_has_both_a_long_and_a_short_spelling() {
    assert_eq!(parse("/takecontrol"), Some(SlashCommand::TakeControl));
    assert_eq!(parse("/take"), Some(SlashCommand::TakeControl));
    assert_eq!(parse("/handoff"), Some(SlashCommand::HandOff));
    assert_eq!(parse("/hand"), Some(SlashCommand::HandOff));
}
