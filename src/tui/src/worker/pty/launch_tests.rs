//! Tests for the launch module.

use super::*;

#[test]
fn the_interactive_argv_suppresses_nothing() {
    // The headless flags exist to hide the interface we are rendering.
    for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
        let args = interactive_args(provider, None, false, None, &[]);
        assert!(args.is_empty(), "{provider:?} paints by default: {args:?}");
    }
}

#[test]
fn each_harness_gets_its_own_permission_bypass_flag() {
    // The two spell it differently and neither name is guessable; both are
    // taken from the installed CLIs' `--help`. Getting one wrong means the
    // harness exits on an unknown argument, so pin the exact strings.
    assert_eq!(
        interactive_args(HarnessProvider::Claude, None, true, None, &[]),
        vec!["--dangerously-skip-permissions"]
    );
    assert_eq!(
        interactive_args(HarnessProvider::Codex, None, true, None, &[]),
        vec!["--dangerously-bypass-approvals-and-sandbox"]
    );
}

#[test]
fn the_bypass_is_off_unless_asked_for() {
    // Running someone's harness without permission checks is not something
    // to arrive at by default in this function; the caller decides.
    for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
        let args = interactive_args(provider, Some("s1"), false, None, &[]);
        assert!(
            !args.iter().any(|a| a.starts_with("--dangerous")),
            "{provider:?}: {args:?}"
        );
    }
}

#[test]
fn the_bypass_precedes_the_session_id_and_extras() {
    // claude parses flags in order and `--session-id` takes a value; a flag
    // landing between them would consume it.
    let args = interactive_args(
        HarnessProvider::Claude,
        Some("abc-123"),
        true,
        None,
        &["--extra".into(), "x".into()],
    );
    assert_eq!(
        args,
        vec![
            "--dangerously-skip-permissions",
            "--session-id",
            "abc-123",
            "--extra",
            "x"
        ]
    );
}

#[test]
fn opencode_needs_its_tui_subcommand() {
    assert_eq!(
        interactive_args(HarnessProvider::Opencode, None, false, None, &[]),
        vec!["tui"]
    );
}

#[test]
fn openhuman_launches_its_native_tui_without_coding_harness_flags() {
    assert_eq!(
        interactive_args(HarnessProvider::Openhuman, None, true, Some("ignored"), &[]),
        vec!["tui"]
    );
}

#[test]
fn extra_args_follow_the_base() {
    let args = interactive_args(
        HarnessProvider::Opencode,
        None,
        false,
        None,
        &["--extra".into(), "x".into()],
    );
    assert_eq!(args, vec!["tui", "--extra", "x"]);
}

#[test]
fn model_is_encoded_with_the_flag_each_harness_expects() {
    // claude takes the long form; codex/opencode take the short one. Matches
    // the headless argv builder's spellings, and the model comes before any
    // extra args so a caller's `extra_args` cannot shadow it.
    assert_eq!(
        interactive_args(HarnessProvider::Claude, None, false, Some("opus"), &[]),
        vec!["--model", "opus"]
    );
    assert_eq!(
        interactive_args(HarnessProvider::Codex, None, false, Some("o3"), &[]),
        vec!["-m", "o3"]
    );
    assert_eq!(
        interactive_args(HarnessProvider::Opencode, None, false, Some("gpt"), &[]),
        vec!["tui", "-m", "gpt"]
    );
}

#[test]
fn an_unset_model_adds_no_flag() {
    let args = interactive_args(HarnessProvider::Claude, None, false, None, &[]);
    assert!(!args.iter().any(|a| a == "--model" || a == "-m"));
}

#[test]
fn submit_is_a_carriage_return_not_a_newline() {
    // `\n` types a literal newline into the composer instead of submitting.
    assert_eq!(submit_sequence(), b"\r");
}

#[test]
fn injected_text_is_bracketed_as_one_paste() {
    let out = bracket_paste("line one\nline two");
    assert!(out.starts_with(b"\x1b[200~"));
    assert!(out.ends_with(b"\x1b[201~"));
    // The embedded newline must survive — bracketing is what stops it
    // submitting a partial prompt.
    assert!(String::from_utf8_lossy(&out).contains("line one\nline two"));
}
