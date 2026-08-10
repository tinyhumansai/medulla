//! What the built-in reporting hooks promise: they observe and never decide,
//! they reach the binary that launched the harness, and they survive a path a
//! shell would otherwise split.

use super::*;
use crate::harness_hooks::{hook_injection, HooksConfig};

#[test]
fn builtins_only_observe() {
    for event in REPORTED_EVENTS {
        assert!(
            !matches!(event, HookEvent::PreToolUse | HookEvent::PermissionRequest),
            "{} can deny a call and must not be installed by default",
            event.as_str()
        );
    }
}

#[test]
fn every_builtin_calls_the_launching_binary() {
    let hooks = hooks("/opt/medulla/bin/medulla");
    assert_eq!(hooks.len(), REPORTED_EVENTS.len());
    for hook in &hooks {
        assert!(hook.builtin);
        assert_eq!(
            hook.command(),
            format!("/opt/medulla/bin/medulla hook {}", hook.event.as_str())
        );
        assert_eq!(hook.timeout(), Some(timeout_for(hook.event)));
        assert!(hook.label.is_some(), "the Hooks page needs a name to show");
    }
}

/// Codex's own hook config docs cap a `SessionEnd` handler's timeout at three
/// seconds; serializing the ordinary five-second budget onto it would hand a
/// default Codex launch an unsupported hook definition for the event it is
/// supposed to report on exit.
#[test]
fn session_end_never_declares_a_timeout_codex_would_refuse() {
    let session_end = hooks("medulla")
        .into_iter()
        .find(|hook| hook.event == HookEvent::SessionEnd)
        .expect("a SessionEnd built-in");
    // Codex refuses a `SessionEnd` hook whose timeout exceeds three seconds;
    // pinning the exact value (rather than a `<=` bound) means this fails
    // loudly if the constant is ever raised past what Codex accepts.
    assert_eq!(session_end.timeout(), Some(3));
}

/// The tests below call `shell_quote_for` directly with an explicit platform
/// rather than through `hooks()` (which quotes for `cfg!(windows)`, i.e.
/// whichever platform actually runs the test): the POSIX and Windows rules
/// both need coverage regardless of which CI runner executes this file.
#[test]
fn a_path_with_spaces_stays_one_word_on_posix() {
    assert_eq!(
        shell_quote_for("/Applications/My Tools/medulla", false),
        "'/Applications/My Tools/medulla'"
    );
}

#[test]
fn an_embedded_single_quote_cannot_break_out_on_posix() {
    assert_eq!(
        shell_quote_for("/tmp/it's here/medulla", false),
        r"'/tmp/it'\''s here/medulla'"
    );
}

#[test]
fn a_path_with_spaces_is_double_quoted_on_windows() {
    assert_eq!(
        shell_quote_for(r"C:\Program Files\medulla", true),
        "\"C:\\Program Files\\medulla\""
    );
}

#[test]
fn an_embedded_double_quote_is_doubled_on_windows() {
    assert_eq!(
        shell_quote_for(r#"C:\Program "Files"\medulla"#, true),
        "\"C:\\Program \"\"Files\"\"\\medulla\""
    );
}

#[test]
fn a_plain_path_needs_no_quoting_on_either_platform() {
    assert_eq!(
        shell_quote_for("/opt/medulla/bin/medulla", false),
        "/opt/medulla/bin/medulla"
    );
    assert_eq!(
        shell_quote_for("/opt/medulla/bin/medulla", true),
        "/opt/medulla/bin/medulla"
    );
}

#[test]
fn notification_is_claude_only_and_raises_no_codex_note() {
    let notification = hooks("medulla")
        .into_iter()
        .find(|hook| hook.event == HookEvent::Notification)
        .expect("a Notification built-in");
    assert_eq!(notification.harnesses, vec![HarnessProvider::Claude]);

    let config = HooksConfig::default().with_builtin(hooks("medulla"));
    let injection = hook_injection(HarnessProvider::Codex, &config);
    assert!(
        injection.dropped.is_empty(),
        "a built-in the operator never declared must not report itself as dropped: {:?}",
        injection.dropped
    );
}

/// The Notification built-in reports only the notification types that mean the
/// harness is blocked on the operator.
///
/// Claude Code fires `notification` for informational events too — above all
/// `idle_prompt`, every time a finished turn returns to the prompt — and that
/// rest is precisely the state the TUI must not light up as "waiting on you",
/// or the badge ticks on every session that merely finished a turn. Those
/// types must never reach the hook log, so the ones the built-in reports stay
/// exactly the operator-answering set. The matcher is exact-value alternation,
/// so splitting on `|` is a faithful reading of it.
#[test]
fn notification_builtin_matches_only_operator_blocking_types() {
    let notification = hooks("medulla")
        .into_iter()
        .find(|hook| hook.event == HookEvent::Notification)
        .expect("a Notification built-in");
    let reported: Vec<&str> = notification.matcher.split('|').collect();

    for waiting in [
        "permission_prompt",
        "elicitation_dialog",
        "agent_needs_input",
    ] {
        assert!(
            reported.contains(&waiting),
            "a '{waiting}' notification is the harness stopped on the operator and must be reported"
        );
    }
    for informational in [
        "idle_prompt",
        "auth_success",
        "elicitation_complete",
        "elicitation_response",
        "agent_completed",
    ] {
        assert!(
            !reported.contains(&informational),
            "an '{informational}' notification is informational or the idle rest and must not be reported"
        );
    }
}

#[test]
fn resolving_twice_is_not_additive() {
    let config = HooksConfig::default().with_builtin(hooks("medulla"));
    let again = config.with_builtin(hooks("medulla"));
    assert_eq!(again.hooks.len(), REPORTED_EVENTS.len());
}

#[test]
fn operator_hooks_survive_resolution_and_come_back_out_alone() {
    let mine = HookSpec {
        event: HookEvent::Stop,
        matcher: "*".to_string(),
        handler: HookHandler::Command {
            command: "notify-send done".to_string(),
            timeout: None,
        },
        harnesses: Vec::new(),
        label: None,
        builtin: false,
    };
    let config = HooksConfig { hooks: vec![mine] }.with_builtin(hooks("medulla"));
    assert_eq!(config.hooks.len(), REPORTED_EVENTS.len() + 1);
    // The operator's own hook runs after the report, and is the only thing a
    // config file should get back.
    let operator = config.operator_hooks();
    assert_eq!(operator.len(), 1);
    assert_eq!(operator[0].command(), "notify-send done");
}
