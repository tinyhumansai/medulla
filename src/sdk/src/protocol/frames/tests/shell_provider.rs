//! Tests that `HarnessProvider::Shell` cannot be reached by dispatch.
//!
//! A shell is in the enum because the operator's own TUI opens one on the same
//! pty machinery as a harness. Nothing else about the system should be able to
//! see it: a shell reads no prompt and reports no completion, so a task frame
//! naming one is a turn that never answers — and a peer that could name one
//! would have a way to run whatever it liked on this host with no harness in
//! between. These are the parses that stand between the two.

use crate::protocol::{
    decode_task_frame, parse_agent_capabilities, HarnessProvider, MEDULLA_TASK_PROTO,
};

#[test]
fn shell_names_itself_but_is_not_dispatchable() {
    assert_eq!(HarnessProvider::from_wire("shell"), Some(HarnessProvider::Shell));
    assert_eq!(HarnessProvider::Shell.as_str(), "shell");
    assert!(!HarnessProvider::Shell.is_dispatchable());
    assert_eq!(HarnessProvider::dispatchable_from_wire("shell"), None);
    assert_eq!(HarnessProvider::flavor_from_wire("shell"), None);
    assert!(!crate::protocol::dispatchable_flavors()
        .iter()
        .any(|(provider, _)| *provider == HarnessProvider::Shell));
}

/// A frame naming a shell decodes as "no preference", so the responder falls
/// back to the harness it would have chosen for itself. Deliberately not a
/// rejected frame: the work is still work, and only the choice of runner is
/// refused.
#[test]
fn a_task_frame_naming_a_shell_falls_back_to_the_host_default() {
    let body = serde_json::json!({
        "proto": MEDULLA_TASK_PROTO,
        "kind": "task",
        "taskId": "t-1",
        "text": "rm -rf /",
        "ts": "2026-07-18T00:00:00.000Z",
        "harness": "shell",
        "provider": "shell",
    })
    .to_string();

    let frame = decode_task_frame(&body).expect("the frame itself is well-formed");

    assert_eq!(frame.harness, None);
    assert_eq!(frame.provider, None);
}

/// A peer's advertised provider list is its claim about what it will run. One
/// naming a shell is offering to take delegated work on a terminal, so the
/// entry is dropped rather than becoming a candidate the orchestrator can pick.
#[test]
fn an_advertised_shell_provider_is_dropped() {
    let capabilities =
        parse_agent_capabilities(r#"{"providers":["claude","shell"]}"#).expect("valid JSON");

    assert_eq!(capabilities.providers, vec![HarnessProvider::Claude]);
}
