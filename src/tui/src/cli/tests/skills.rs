//! Unit tests for `medulla skills` argument parsing.
//!
//! Every flag here decides where files are written, so the coverage is
//! deliberately flag-by-flag rather than a couple of happy paths: a flag that
//! parses to the wrong thing installs to the wrong place, and the operator
//! finds out by not having a skill.

use medulla::workflows::skills::{SkillScope, SkillTarget};

use super::argv;
use crate::cli::*;

/// Parse, asserting the args were accepted.
fn parse(parts: &[&str]) -> SkillsArgs {
    parse_skills_args(&argv(parts)).expect("args should parse")
}

#[test]
fn skills_is_a_top_level_command_under_either_spelling() {
    assert_eq!(parse_command(&argv(&["skills"])), Command::Skills);
    assert_eq!(parse_command(&argv(&["skill"])), Command::Skills);
    assert_eq!(
        parse_command(&argv(&["skills", "install"])),
        Command::Skills
    );
}

#[test]
fn a_bare_invocation_lists() {
    let parsed = parse(&[]);
    assert_eq!(parsed.action, SkillsAction::List);
    assert!(parsed.ids.is_empty());
    assert_eq!(parsed.targets, None);
    assert_eq!(parsed.scope, SkillScope::User);
    assert_eq!(parsed.tools, "run");
    assert!(!parsed.json);
    assert!(!parsed.dry_run);
}

#[test]
fn every_verb_reaches_its_action() {
    for (verb, action) in [
        ("list", SkillsAction::List),
        ("install", SkillsAction::Install),
        ("add", SkillsAction::Install),
        ("sync", SkillsAction::Sync),
        ("refresh", SkillsAction::Sync),
        ("uninstall", SkillsAction::Uninstall),
        ("remove", SkillsAction::Uninstall),
        ("rm", SkillsAction::Uninstall),
    ] {
        assert_eq!(parse(&[verb]).action, action, "verb {verb}");
    }
}

#[test]
fn bare_words_after_the_verb_are_workflow_ids() {
    let parsed = parse(&["install", "babysit", "sweep"]);
    assert_eq!(parsed.action, SkillsAction::Install);
    assert_eq!(parsed.ids, vec!["babysit".to_string(), "sweep".to_string()]);
}

#[test]
fn the_verb_is_never_mistaken_for_an_id() {
    // Otherwise `skills install` would look for a workflow called "install".
    assert!(parse(&["install"]).ids.is_empty());
    assert!(parse(&["uninstall"]).ids.is_empty());
}

#[test]
fn harness_accepts_a_comma_joined_list() {
    assert_eq!(
        parse(&["install", "--harness", "claude,codex"]).targets,
        Some(vec![SkillTarget::Claude, SkillTarget::Codex])
    );
}

#[test]
fn harness_is_repeatable_and_the_two_spellings_compose() {
    let parsed = parse(&[
        "install",
        "--harness",
        "claude",
        "--harness",
        "codex,generic",
    ]);
    assert_eq!(
        parsed.targets,
        Some(vec![
            SkillTarget::Claude,
            SkillTarget::Codex,
            SkillTarget::Generic
        ])
    );
}

#[test]
fn a_repeated_harness_is_only_written_once() {
    // Duplicates fall out of accepting both spellings; writing the file twice
    // would double every line of the report.
    assert_eq!(
        parse(&[
            "install",
            "--harness",
            "claude,claude",
            "--harness",
            "claude"
        ])
        .targets,
        Some(vec![SkillTarget::Claude])
    );
}

#[test]
fn harness_all_selects_every_target() {
    assert_eq!(
        parse(&["install", "--harness", "all"]).targets,
        Some(SkillTarget::ALL.to_vec())
    );
    // Case-insensitive, like every other enum value here.
    assert_eq!(
        parse(&["install", "--harness", "ALL"]).targets,
        Some(SkillTarget::ALL.to_vec())
    );
}

#[test]
fn no_harness_flag_leaves_the_choice_to_the_runner() {
    // `None` and "an empty list" mean different things: the first is "detect
    // what exists", the second would be "write nothing".
    assert_eq!(parse(&["install"]).targets, None);
}

#[test]
fn an_unknown_harness_is_rejected_by_name() {
    let err = parse_skills_args(&argv(&["install", "--harness", "cluade"]))
        .expect_err("a mistyped harness must not be ignored");
    assert!(err.contains("cluade"), "{err}");
    assert!(err.contains("claude"), "{err}");
    assert!(err.contains("codex"), "{err}");
}

#[test]
fn a_valueless_harness_is_rejected() {
    let err = parse_skills_args(&argv(&["install", "--harness"]))
        .expect_err("--harness with no value must not silently mean nothing");
    assert!(err.contains("--harness"), "{err}");
}

#[test]
fn scope_parses_both_values_and_rejects_the_rest() {
    assert_eq!(
        parse(&["install", "--scope", "user"]).scope,
        SkillScope::User
    );
    assert_eq!(
        parse(&["install", "--scope", "project"]).scope,
        SkillScope::Project
    );
    let err = parse_skills_args(&argv(&["install", "--scope", "global"]))
        .expect_err("an unknown scope must be rejected");
    assert!(err.contains("global"), "{err}");
}

#[test]
fn dir_overrides_the_computed_root() {
    assert_eq!(
        parse(&["install", "--dir", "/tmp/scratch"]).dir.as_deref(),
        Some("/tmp/scratch")
    );
    let err = parse_skills_args(&argv(&["install", "--dir"]))
        .expect_err("--dir with no value must be rejected");
    assert!(err.contains("--dir"), "{err}");
}

#[test]
fn tools_defaults_to_run_and_accepts_full() {
    assert_eq!(parse(&["install"]).tools, "run");
    assert_eq!(parse(&["install", "--tools", "full"]).tools, "full");
    assert_eq!(parse(&["install", "--tools", "RUN"]).tools, "run");
    let err = parse_skills_args(&argv(&["install", "--tools", "everything"]))
        .expect_err("an unknown tool mode must be rejected");
    assert!(err.contains("everything"), "{err}");
    assert!(err.contains("run"), "{err}");
}

#[test]
fn the_boolean_flags_are_all_read() {
    let parsed = parse(&[
        "install",
        "babysit",
        "--with-mcp",
        "--with-commands",
        "--dry-run",
        "--json",
    ]);
    assert!(parsed.with_mcp);
    assert!(parsed.with_commands);
    assert!(parsed.dry_run);
    assert!(parsed.json);
    assert!(!parsed.prune);
}

#[test]
fn prune_is_read_for_sync() {
    let parsed = parse(&["sync", "--prune", "--harness", "codex"]);
    assert_eq!(parsed.action, SkillsAction::Sync);
    assert!(parsed.prune);
    assert_eq!(parsed.targets, Some(vec![SkillTarget::Codex]));
}

#[test]
fn prune_with_named_ids_is_refused_rather_than_guessed_at() {
    // Honouring both would prune against a one-id keep set, deleting the skills
    // for every workflow the operator did not name. The parser refuses instead.
    let err = parse_skills_args(&argv(&["sync", "babysit", "--prune"]))
        .expect_err("`sync <id> --prune` must not be accepted");
    assert!(err.contains("babysit"), "{err}");
    assert!(err.contains("--prune"), "{err}");
    // Ids alone, and --prune alone, are both still fine.
    assert_eq!(parse(&["sync", "babysit"]).ids, vec!["babysit".to_string()]);
    assert!(parse(&["sync", "--prune"]).prune);
    // Only `sync` prunes, so the flag is inert (not an error) elsewhere.
    assert_eq!(parse(&["install", "babysit", "--prune"]).ids.len(), 1);
}

#[test]
fn all_is_read_for_uninstall_and_is_off_by_default() {
    // Without it a bare `uninstall` is refused by the runner: "remove every
    // managed skill" is one typo away from "remove this one".
    assert!(!parse(&["uninstall"]).all);
    let parsed = parse(&["uninstall", "--all"]);
    assert_eq!(parsed.action, SkillsAction::Uninstall);
    assert!(parsed.all);
    assert!(parsed.ids.is_empty());
}

#[test]
fn unknown_flags_are_tolerated_the_way_the_other_parsers_tolerate_them() {
    let parsed = parse(&["list", "--verbose", "--json"]);
    assert_eq!(parsed.action, SkillsAction::List);
    assert!(parsed.json);
}

#[test]
fn help_text_documents_the_skills_command() {
    let help = help_text();
    assert!(help.contains("medulla skills"));
    assert!(help.contains("--with-mcp"));
    assert!(help.contains("--harness"));
    // The destructive form has to be discoverable from the help alone.
    assert!(help.contains("--all"), "{help}");
}
