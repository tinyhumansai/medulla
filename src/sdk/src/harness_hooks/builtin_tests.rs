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
        assert_eq!(hook.timeout(), Some(REPORT_TIMEOUT_SECS));
        assert!(hook.label.is_some(), "the Hooks page needs a name to show");
    }
}

#[test]
fn a_path_with_spaces_stays_one_word() {
    let hooks = hooks("/Applications/My Tools/medulla");
    let start = &hooks[0];
    assert_eq!(
        start.command(),
        format!(
            "'/Applications/My Tools/medulla' hook {}",
            start.event.as_str()
        )
    );
}

#[test]
fn an_embedded_quote_cannot_break_out() {
    let hooks = hooks("/tmp/it's here/medulla");
    assert!(hooks[0]
        .command()
        .starts_with(r"'/tmp/it'\''s here/medulla' hook "));
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
