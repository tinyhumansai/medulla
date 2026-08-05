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

/// Every event accepts its camelCase spelling in config, because that is the
/// spelling every *other* key in `medulla.tui.toml` teaches — and getting it
/// wrong used to fail the whole config load, not just the hook.
///
/// Derived from `as_str` rather than listed by hand: a list would be one more
/// thing to forget when a variant is added, and the point of the alias is that
/// no variant is missing one.
#[test]
fn every_event_deserializes_from_its_camel_case_spelling() {
    for event in HookEvent::ALL {
        let pascal = event.as_str();
        let mut chars = pascal.chars();
        let camel: String = chars
            .next()
            .map(|first| first.to_ascii_lowercase().to_string() + chars.as_str())
            .expect("no event name is empty");

        let parsed: HookEvent = serde_json::from_value(serde_json::json!(camel))
            .unwrap_or_else(|err| panic!("`{camel}` must deserialize to {event:?}: {err}"));
        assert_eq!(parsed, event);

        // The canonical spelling keeps working, and stays the one written back
        // out — the harnesses only answer to it.
        let parsed: HookEvent = serde_json::from_value(serde_json::json!(pascal))
            .unwrap_or_else(|err| panic!("`{pascal}` must still deserialize: {err}"));
        assert_eq!(parsed, event);
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!(pascal)
        );
    }
}

/// The aliases widen the vocabulary by exactly one spelling per event, not into
/// a free-for-all: a name that is neither spelling is still refused, so a typo
/// is still reported rather than silently dropping a hook the operator believes
/// is installed.
#[test]
fn a_misspelled_event_is_still_refused() {
    for bad in ["pre_tool_use", "POSTTOOLUSE", "postToolUsage", "Nope", ""] {
        assert!(
            serde_json::from_value::<HookEvent>(serde_json::json!(bad)).is_err(),
            "{bad:?} is not an event and must not parse"
        );
    }
}

#[test]
fn codex_omits_notification_and_session_end() {
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
fn codex_delivery_passes_an_inline_toml_override_without_bypassing_trust() {
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let injection = hook_injection(HarnessProvider::Codex, &hooks);

    assert_eq!(injection.args[0], "-c");
    assert!(injection.args[1].starts_with("hooks="));
    assert!(injection.args[1].contains("SessionStart"));
    assert!(injection.args[1].contains("echo hi"));
    assert!(!injection
        .args
        .iter()
        .any(|arg| arg.contains("bypass-hook-trust")));
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
fn codex_installs_session_end_because_the_cli_does_raise_that_event() {
    // Verified against Codex 0.146 by running a `SessionEnd` command hook
    // end-to-end: it fires. `session_end` is also in the binary's own hook-event
    // vocabulary and in Codex's published hook documentation. Dropping it would
    // silently disable a hook that works.
    let hooks = config(vec![hook(HookEvent::SessionEnd, "*", "cleanup")]);
    let injection = hook_injection(HarnessProvider::Codex, &hooks);

    assert!(injection.dropped.is_empty());
    assert!(injection.args[1].contains("SessionEnd"));
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
    assert!(!args.iter().any(|arg| arg.contains("bypass-hook-trust")));
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

#[test]
fn codex_reports_that_its_hooks_are_inert_until_trusted() {
    // Codex skips hooks absent from its trust store, and a per-spawn injection
    // always is. Medulla does not pass `--dangerously-bypass-hook-trust`,
    // because that flag is invocation-wide and would also authorize whatever
    // hooks the checked-out repository declares. The cost is that the feature
    // does nothing until the operator trusts it — which must be said, or this
    // module reproduces the exact silent failure it exists to prevent.
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let injection = hook_injection(HarnessProvider::Codex, &hooks);

    assert!(
        !injection
            .args
            .iter()
            .any(|arg| arg.contains("bypass-hook-trust")),
        "the invocation-wide trust bypass must never be passed",
    );
    assert_eq!(injection.warnings.len(), 1);
    assert!(injection.warnings[0].contains("trust"));
    // The warning reaches callers through the same channel dropped hooks do.
    assert!(injection.notes().iter().any(|note| note.contains("trust")));
}

#[test]
fn claude_needs_no_trust_warning() {
    // Claude Code runs `--settings` hooks without prompting, so there is nothing
    // for the operator to do and nothing to warn about.
    let hooks = config(vec![hook(HookEvent::SessionStart, "*", "echo hi")]);
    let injection = hook_injection(HarnessProvider::Claude, &hooks);
    assert!(injection.warnings.is_empty());
}
