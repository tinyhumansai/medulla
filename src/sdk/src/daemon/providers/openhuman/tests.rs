//! Unit tests for the embedded-core provider's routing and reply decoding.
//!
//! Deliberately no test that actually runs a turn: doing so would boot the
//! process-wide core, which starts background services and binds a workspace —
//! see [`crate::core_host::shared`] on why exactly one of those may exist per
//! process. What *is* testable here is every decision made around the call.

use std::collections::HashMap;

use serde_json::json;

use crate::protocol::{HarnessProvider, HarnessTransport};
use crate::sessions::SessionClass;

use super::super::types::{Abort, RunTaskOptions};
use super::run::{reply_text, uses_embedded_core};

/// Options naming `provider`, with everything else at its least interesting.
fn options(provider: HarnessProvider) -> RunTaskOptions {
    RunTaskOptions {
        provider,
        transport: HarnessTransport::Cli,
        prompt: "do the thing".to_string(),
        cwd: ".".to_string(),
        env: HashMap::new(),
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        conversation: String::new(),
        session_class: SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        abort: Abort::new(),
        router: None,
        attribution: false,
        hooks: Default::default(),
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

/// OpenHuman takes the in-process path; every CLI provider takes the spawn
/// path. This is the whole of the routing decision, and getting it wrong in
/// either direction is silent — a CLI routed here would never spawn, and
/// OpenHuman routed to the spawn seam would look for a binary that does not
/// exist.
#[test]
fn only_openhuman_runs_on_the_embedded_core() {
    assert!(uses_embedded_core(&options(HarnessProvider::Openhuman)));
    assert!(!uses_embedded_core(&options(HarnessProvider::Claude)));
    assert!(!uses_embedded_core(&options(HarnessProvider::Codex)));
    assert!(!uses_embedded_core(&options(HarnessProvider::Opencode)));
}

/// A handler that logged nothing returns the bare value.
#[test]
fn a_bare_string_reply_is_the_answer() {
    assert_eq!(reply_text(json!("all done")), "all done");
}

/// A handler that logged anything wraps the answer, and whether it logs is an
/// implementation detail that can change without notice — so both shapes are
/// accepted rather than the one it emits today.
#[test]
fn a_logged_outcome_envelope_is_unwrapped() {
    let wire = json!({ "result": "all done", "logs": ["agent chat completed"] });
    assert_eq!(reply_text(wire), "all done");
}

/// An object that merely *has* a `result` field is not an envelope. Unwrapping
/// it would hand back a fragment of the answer as the whole of it.
#[test]
fn a_value_that_is_not_an_envelope_is_left_alone() {
    let wire = json!({ "result": "partial", "other": 1 });
    assert_eq!(reply_text(wire.clone()), wire.to_string());

    let wire = json!({ "result": "partial", "logs": 3 });
    assert_eq!(reply_text(wire.clone()), wire.to_string());
}

/// A structural answer is rendered rather than reported as empty — this method
/// returns a string today, and a build that meets something else should show
/// it.
#[test]
fn a_structural_reply_is_rendered_rather_than_dropped() {
    assert_eq!(reply_text(json!({ "a": 1 })), "{\"a\":1}");
}

/// An already-aborted task fails before the core is touched. Asserted because
/// the alternative is booting a core to run work that was already cancelled.
#[tokio::test]
async fn an_aborted_task_fails_without_booting_a_core() {
    let options = options(HarnessProvider::Openhuman);
    options.abort.abort();

    let error = super::run_openhuman_task(options)
        .await
        .expect_err("an aborted task must not run");
    assert!(error.contains("aborted before start"), "{error}");
}
