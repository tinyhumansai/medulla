//! Unit tests for ACP-transport hook delivery and its local PostToolUse runner.
//!
//! What the delivery tests pin is the *shape on the wire*, key by key.
//! `claude-agent-acp` reads one specific nested path out of `_meta` and ignores
//! everything else without complaining, so a delivery that is merely well-formed
//! JSON is indistinguishable from one that works. These assertions are the
//! record of the path that was read out of `claude-agent-acp` 0.65.0 and then
//! confirmed by watching a hook delivered that way fire.

use serde_json::{json, Value};

use crate::harness_hooks::types::{HookEvent, HookHandler, HookSpec, HooksConfig};
use crate::protocol::HarnessProvider;

use super::delivery;
use super::runner::{matches_tool, normalized_input, tool_matcher_candidates};

/// One `PostToolUse` hook running `command`, applying to every harness.
fn hook(command: &str) -> HookSpec {
    HookSpec {
        event: HookEvent::PostToolUse,
        matcher: "*".to_string(),
        handler: HookHandler::Command {
            command: command.to_string(),
            timeout: None,
        },
        harnesses: Vec::new(),
        label: None,
        builtin: false,
    }
}

/// A config holding just that hook.
fn one_hook(command: &str) -> HooksConfig {
    HooksConfig {
        hooks: vec![hook(command)],
    }
}

// ---------------------------------------------------------------------------
// Claude `_meta` delivery
// ---------------------------------------------------------------------------

/// The exact path `claude-agent-acp` reads: `_meta.claudeCode.options.settings`,
/// which it forwards to the Claude Agent SDK as the equivalent of `--settings`.
/// Anything nested one level off is silently ignored by that server, so the path
/// itself is the test.
#[test]
fn claude_carries_hooks_in_the_session_meta_settings_option() {
    let delivered = delivery(HarnessProvider::Claude, &one_hook("auto-commit --hook"));

    let meta = delivered
        .session_meta
        .expect("Claude takes hooks via _meta");
    let settings = meta
        .get("claudeCode")
        .and_then(|claude| claude.get("options"))
        .and_then(|options| options.get("settings"))
        .expect("_meta.claudeCode.options.settings is the path the ACP server reads");
    assert_eq!(
        settings,
        &json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "*",
                    "hooks": [{ "type": "command", "command": "auto-commit --hook" }],
                }],
            },
        }),
    );
    assert!(
        delivered.notes.is_empty(),
        "a fully delivered hook needs no explanation: {:?}",
        delivered.notes
    );
}

/// Attribution is applied to ACP runs through the `prepare-commit-msg` hook
/// environment, so carrying it in the settings document as well would put the
/// trailer on every commit twice.
#[test]
fn claude_settings_carry_hooks_only() {
    let delivered = delivery(HarnessProvider::Claude, &one_hook("auto-commit --hook"));

    let meta = delivered
        .session_meta
        .expect("Claude takes hooks via _meta");
    let Value::Object(fields) = meta["claudeCode"]["options"]["settings"].clone() else {
        panic!("settings is an object");
    };
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["hooks"]);
}

/// Several hooks on one event arrive as one document, in config order — the
/// same folding the CLI path does, since both build it from
/// [`super::super::native::hook_document`].
#[test]
fn claude_settings_carry_every_applicable_hook() {
    let hooks = HooksConfig {
        hooks: vec![hook("first"), hook("second")],
    };

    let delivered = delivery(HarnessProvider::Claude, &hooks);

    let meta = delivered
        .session_meta
        .expect("Claude takes hooks via _meta");
    let commands = meta["claudeCode"]["options"]["settings"]["hooks"]["PostToolUse"][0]["hooks"]
        .as_array()
        .expect("one matcher group holding both handlers")
        .iter()
        .map(|handler| handler["command"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(commands, ["first", "second"]);
}

// ---------------------------------------------------------------------------
// Codex local PostToolUse fallback
// ---------------------------------------------------------------------------

/// Codex app-server does not execute lifecycle hooks, but ACP reports each
/// completed tool call to Medulla.  The transport therefore keeps the
/// observation-only PostToolUse hook for Medulla to execute locally, and says
/// that the local fallback is observation-only for a non-Medulla hook.
#[test]
fn codex_runs_post_tool_use_locally() {
    let delivered = delivery(HarnessProvider::Codex, &one_hook("auto-commit --hook"));

    assert!(delivered.session_meta.is_none());
    assert_eq!(delivered.local_post_tool_use.len(), 1);
    assert_eq!(
        delivered.local_post_tool_use[0].command(),
        "auto-commit --hook"
    );
    assert!(
        delivered
            .notes
            .iter()
            .any(|note| note.contains("observation-only")),
        "an operator hook run through the fallback must say it is observation-only: {:#?}",
        delivered.notes
    );
}

/// No declared hook means no delivery and nothing to explain — an ACP spawn
/// identical to what it was before this module existed.
#[test]
fn no_hooks_delivers_nothing() {
    let delivered = delivery(HarnessProvider::Claude, &HooksConfig::default());

    assert!(delivered.is_empty());
    assert!(delivered.notes.is_empty());
}

/// A hook the operator restricted to other harnesses is not carried, and is not
/// reported as undeliverable either — it was never meant for this session.
#[test]
fn a_hook_restricted_to_other_harnesses_is_not_delivered() {
    let mut spec = hook("codex-only");
    spec.harnesses = vec![HarnessProvider::Codex];

    let delivered = delivery(HarnessProvider::Claude, &HooksConfig { hooks: vec![spec] });

    assert!(delivered.is_empty());
    assert!(delivered.notes.is_empty());
}

/// OpenCode's hook surface is a scripted plugin API Medulla does not translate
/// to. Nothing is delivered, and the operator is told which hook was lost rather
/// than left to infer it from a harness that quietly never checkpoints.
#[test]
fn opencode_delivers_nothing_but_says_so() {
    let delivered = delivery(HarnessProvider::Opencode, &one_hook("auto-commit --hook"));

    assert!(delivered.is_empty());
    assert!(
        delivered
            .notes
            .iter()
            .any(|note| note.contains("PostToolUse")),
        "the undelivered hook must be named: {:?}",
        delivered.notes
    );
}

/// Delivery reads nothing but the hooks it was handed, so two calls with the
/// same config produce the same thing — which is what lets the Hooks page
/// describe an ACP spawn without performing one.
#[test]
fn delivery_is_repeatable() {
    let hooks = one_hook("auto-commit --hook");

    let first = delivery(HarnessProvider::Claude, &hooks);
    let second = delivery(HarnessProvider::Claude, &hooks);

    assert_eq!(first, second);
}

// ---------------------------------------------------------------------------
// Runner helpers
// ---------------------------------------------------------------------------

#[test]
fn matcher_preserves_codex_match_all_semantics() {
    assert!(matches_tool("*", "apply_patch"));
    assert!(matches_tool("^Bash$", "Bash"));
    assert!(!matches_tool("^Bash$", "apply_patch"));
}

/// A matcher written for Codex's native hook vocabulary still selects an ACP
/// call: the ACP `kind` is translated to the native tool names it corresponds
/// to, so `Bash` matches an `execute` and `Edit|Write` matches an `edit`.
#[test]
fn matcher_translates_acp_kinds_to_native_codex_tool_names() {
    assert!(matches_tool("Bash", "execute"));
    assert!(matches_tool("^Bash$", "execute"));
    assert!(matches_tool("Edit|Write", "edit"));
    assert!(matches_tool("^View$", "read"));
    assert!(!matches_tool("Bash", "edit"));
    assert!(!matches_tool("^Bash$", "read"));
}

#[test]
fn matcher_keeps_the_raw_kind_as_a_candidate() {
    assert!(matches_tool("execute", "execute"));
    assert!(tool_matcher_candidates("execute").contains(&"execute".to_string()));
    assert_eq!(tool_matcher_candidates("think"), vec!["think"]);
}

#[test]
fn normalizes_acp_path_for_codex_hook_consumers() {
    assert_eq!(
        normalized_input(&json!({"path":"a.rs"}))["file_path"],
        "a.rs"
    );
    assert_eq!(
        normalized_input(&json!({"filePath":"b.rs"}))["file_path"],
        "b.rs"
    );
    assert_eq!(
        normalized_input(&json!({"file_path":"c.rs"}))["file_path"],
        "c.rs"
    );
}
