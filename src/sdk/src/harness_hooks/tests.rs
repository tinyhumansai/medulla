//! Unit tests for the hook vocabulary, the shared document, and each harness's
//! delivery. The delivery assertions pin the exact flags and JSON/TOML spelling
//! that were verified against Claude Code 2.1.221 and Codex 0.146, so a change
//! that would stop hooks firing fails here rather than in production silence.

use serde_json::{json, Value};

use super::native::{hook_document, to_inline_toml};
use super::*;
use crate::protocol::HarnessProvider;

fn hook(event: HookEvent, matcher: &str, command: &str) -> HookSpec {
    HookSpec {
        event,
        matcher: matcher.to_string(),
        handler: HookHandler::Command {
            command: command.to_string(),
            timeout: None,
        },
        harnesses: Vec::new(),
    }
}

fn config(hooks: Vec<HookSpec>) -> HooksConfig {
    HooksConfig { hooks }
}

#[test]
fn event_wire_names_round_trip() {
    for event in HookEvent::ALL {
        assert_eq!(HookEvent::from_wire(event.as_str()), Some(event));
    }
    assert_eq!(HookEvent::from_wire("pre_tool_use"), None);
    assert_eq!(HookEvent::from_wire("Nope"), None);
}

#[test]
fn notification_is_the_only_claude_codex_divergence() {
    for event in HookEvent::ALL {
        assert!(event.supported_by(HarnessProvider::Claude), "{event:?}");
        assert_eq!(
            event.supported_by(HarnessProvider::Codex),
            event != HookEvent::Notification,
            "{event:?}",
        );
        assert!(!event.supported_by(HarnessProvider::Opencode), "{event:?}");
        assert!(!event.supported_by(HarnessProvider::Openhuman), "{event:?}");
    }
}

#[test]
fn document_folds_same_event_and_matcher_into_one_group_in_order() {
    let hooks = config(vec![
        hook(HookEvent::PostToolUse, "Edit", "first"),
        hook(HookEvent::PostToolUse, "Edit", "second"),
        hook(HookEvent::PostToolUse, "Bash", "other"),
    ]);
    let applicable = hooks.for_provider(HarnessProvider::Claude);
    let document = hook_document(&applicable);

    assert_eq!(
        document,
        json!({
            "PostToolUse": [
                {
                    "matcher": "Edit",
                    "hooks": [
                        {"type": "command", "command": "first"},
                        {"type": "command", "command": "second"},
                    ],
                },
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "other"}],
                },
            ]
        })
    );
}

#[test]
fn document_carries_a_declared_timeout_and_omits_an_absent_one() {
    let mut with_timeout = hook(HookEvent::Stop, "*", "slow");
    with_timeout.handler = HookHandler::Command {
        command: "slow".into(),
        timeout: Some(30),
    };
    let hooks = config(vec![with_timeout]);
    let document = hook_document(&hooks.for_provider(HarnessProvider::Claude));
    let handler = &document["Stop"][0]["hooks"][0];
    assert_eq!(handler["timeout"], json!(30));

    let plain = config(vec![hook(HookEvent::Stop, "*", "fast")]);
    let document = hook_document(&plain.for_provider(HarnessProvider::Claude));
    assert!(document["Stop"][0]["hooks"][0].get("timeout").is_none());
}

#[test]
fn harness_restriction_selects_only_named_providers() {
    let mut claude_only = hook(HookEvent::PreToolUse, "*", "guard");
    claude_only.harnesses = vec![HarnessProvider::Claude];
    let hooks = config(vec![claude_only]);

    assert_eq!(hooks.for_provider(HarnessProvider::Claude).len(), 1);
    assert!(hooks.for_provider(HarnessProvider::Codex).is_empty());
    // Restricted away, so not "dropped" — Codex was never a target.
    assert!(hook_injection(HarnessProvider::Codex, &hooks)
        .dropped
        .is_empty());
}

#[test]
fn claude_delivery_passes_one_settings_flag_with_the_hook_document() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let injection = hook_injection(HarnessProvider::Claude, &hooks);

    assert_eq!(injection.args.len(), 2);
    assert_eq!(injection.args[0], "--settings");
    let settings: Value = serde_json::from_str(&injection.args[1]).expect("valid JSON");
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        json!("echo hi")
    );
    assert!(injection.dropped.is_empty());
}

#[test]
fn codex_delivery_passes_an_inline_toml_override_and_the_trust_bypass() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let injection = hook_injection(HarnessProvider::Codex, &hooks);

    assert_eq!(injection.args[0], "-c");
    assert_eq!(
        injection.args[1],
        r#"hooks={"SessionStart"=[{"matcher"="*","hooks"=[{"type"="command","command"="echo hi"}]}]}"#
    );
    // Without this, Codex silently skips every hook it has not already trusted.
    assert_eq!(injection.args[2], "--dangerously-bypass-hook-trust");
}

#[test]
fn codex_drops_notification_with_a_reason_and_keeps_the_rest() {
    let hooks = config(vec![
        hook(HookEvent::Notification, "*", "notify"),
        hook(HookEvent::SessionStart, "*", "start"),
    ]);
    let injection = hook_injection(HarnessProvider::Codex, &hooks);

    assert_eq!(injection.dropped.len(), 1);
    assert_eq!(injection.dropped[0].event, HookEvent::Notification);
    assert!(injection.dropped[0].reason.contains("does not raise"));
    // The supported hook still ships.
    assert!(injection.args[1].contains("SessionStart"));
}

#[test]
fn unadapted_providers_report_every_hook_rather_than_ignoring_it() {
    let hooks = config(vec![hook(HookEvent::PostToolUse, "*", "checkpoint")]);
    for provider in [HarnessProvider::Opencode, HarnessProvider::Openhuman] {
        let injection = hook_injection(provider, &hooks);
        assert!(injection.args.is_empty(), "{provider:?}");
        assert_eq!(injection.dropped.len(), 1, "{provider:?}");
    }
}

#[test]
fn no_hooks_means_no_injection() {
    let injection = hook_injection(HarnessProvider::Claude, &HooksConfig::default());
    assert!(injection.is_empty());
    assert!(injection.dropped.is_empty());
}

#[test]
fn claude_launch_args_merge_attribution_and_hooks_into_a_single_settings_flag() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let (args, _) = launch_args(HarnessProvider::Claude, true, &hooks);

    assert_eq!(
        args.iter().filter(|arg| *arg == "--settings").count(),
        1,
        "a second --settings would silently replace the first",
    );
    let settings: Value = serde_json::from_str(&args[1]).expect("valid JSON");
    assert_eq!(
        settings["attribution"]["commit"],
        json!(crate::attribution::attribution_trailer())
    );
    assert!(settings["hooks"]["SessionStart"].is_array());
}

#[test]
fn claude_launch_args_carry_hooks_even_with_attribution_off() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let (args, _) = launch_args(HarnessProvider::Claude, false, &hooks);

    let settings: Value = serde_json::from_str(&args[1]).expect("valid JSON");
    assert!(settings.get("attribution").is_none());
    assert!(settings["hooks"]["SessionStart"].is_array());
}

#[test]
fn claude_launch_args_are_empty_when_nothing_is_configured() {
    let (args, _) = launch_args(HarnessProvider::Claude, false, &HooksConfig::default());
    assert!(args.is_empty());
}

#[test]
fn codex_launch_args_keep_attribution_and_hooks_side_by_side() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let (args, _) = launch_args(HarnessProvider::Codex, true, &hooks);
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"--dangerously-bypass-hook-trust".to_string()));
}

#[test]
fn inline_toml_escapes_shell_commands_so_they_cannot_break_the_override() {
    let command = r#"sh -c "echo \a	b""#;
    let encoded = to_inline_toml(&json!({ "command": command }));
    assert_eq!(
        encoded, r#"{"command"="sh -c \"echo \\a\tb\""}"#,
        "quotes, backslashes and tabs must survive as escapes",
    );
}

#[test]
fn inline_toml_encodes_the_scalar_shapes_a_document_can_hold() {
    assert_eq!(to_inline_toml(&json!(30)), "30");
    assert_eq!(to_inline_toml(&json!(true)), "true");
    assert_eq!(to_inline_toml(&json!([1, 2])), "[1,2]");
    // Null has no TOML spelling, so it is omitted rather than encoded.
    assert_eq!(to_inline_toml(&json!({ "a": null, "b": 1 })), r#"{"b"=1}"#);
    assert_eq!(to_inline_toml(&json!([null, 1])), "[1]");
}

#[test]
fn hook_spec_deserializes_from_the_config_spelling() {
    let parsed: HooksConfig = serde_json::from_value(json!([
        {
            "event": "PostToolUse",
            "matcher": "Edit|Write",
            "type": "command",
            "command": "medulla checkpoint",
            "timeout": 15,
            "harnesses": ["claude", "codex"],
        },
        // matcher and harnesses are optional.
        {"event": "SessionStart", "type": "command", "command": "echo start"},
    ]))
    .expect("config parses");

    assert_eq!(parsed.hooks.len(), 2);
    assert_eq!(parsed.hooks[0].event, HookEvent::PostToolUse);
    assert_eq!(parsed.hooks[0].matcher, "Edit|Write");
    assert_eq!(parsed.hooks[0].timeout(), Some(15));
    assert_eq!(parsed.hooks[0].command(), "medulla checkpoint");
    assert_eq!(
        parsed.hooks[0].harnesses,
        vec![HarnessProvider::Claude, HarnessProvider::Codex]
    );
    // The default matcher matches every tool.
    assert_eq!(parsed.hooks[1].matcher, "*");
    assert!(parsed.hooks[1].harnesses.is_empty());
}

#[test]
fn every_spawn_seam_uses_the_merged_launch_builder() {
    // Each of these seams launches a harness CLI, and each one previously called
    // `attribution_args` directly — which is exactly how a configured hook came
    // to be silently ignored on the interactive paths. Pin them: a new seam that
    // reaches for `attribution_args` instead of `launch_args` fails here.
    let seams = [
        "src/sdk/src/wrapper/run/mod.rs",
        "src/sdk/src/daemon/providers/execute.rs",
        "src/tui/src/worker/executor/run.rs",
        "src/tui/src/ui/harness_pane/spawn.rs",
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    for seam in seams {
        let source = std::fs::read_to_string(root.join(seam))
            .unwrap_or_else(|error| panic!("{seam}: {error}"));
        assert!(
            source.contains("harness_hooks::launch_args"),
            "{seam} must build its argv with harness_hooks::launch_args",
        );
        assert!(
            !source.contains("attribution::attribution_args"),
            "{seam} still calls attribution_args directly, so configured hooks \
             would not reach the harness it launches",
        );
    }
}
