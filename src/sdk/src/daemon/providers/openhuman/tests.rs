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
use super::run::{reply_text, turn_workspace_root, uses_embedded_core};

/// Options naming `provider`, with everything else at its least interesting.
fn options(provider: HarnessProvider) -> RunTaskOptions {
    RunTaskOptions {
        origin: super::super::types::RunTaskOrigin::DelegatedTask,
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

/// A dispatch that named no working directory grants no root: the turn stays on
/// the core's own workspace rather than on whatever the process happened to be
/// standing in.
#[test]
fn an_unset_working_directory_grants_no_root() {
    assert!(turn_workspace_root("").is_none());
    assert!(turn_workspace_root("   ").is_none());
}

/// A path that does not exist is a host's mistake, not a reason to fail the
/// turn — and granting a root nothing resolves under would be meaningless.
#[test]
fn a_missing_working_directory_grants_no_root() {
    let missing = std::env::temp_dir().join("medulla-openhuman-not-here-6f1a2b");
    assert!(turn_workspace_root(missing.to_str().unwrap()).is_none());
}

/// A file is not a workspace. Caught here rather than left to fail once per
/// tool call inside the turn.
#[test]
fn a_file_is_not_a_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").expect("write");
    assert!(turn_workspace_root(file.to_str().unwrap()).is_none());
}

/// The run's checkout resolves to an absolute, canonical path — the form the
/// core's containment check compares against, so a symlinked or `..`-laden
/// path still contains the files written under it.
#[test]
fn the_runs_checkout_resolves_to_a_canonical_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("checkout");
    std::fs::create_dir(&nested).expect("mkdir");
    let indirect = nested.join("..").join("checkout");
    let resolved = turn_workspace_root(indirect.to_str().unwrap()).expect("an existing directory");
    assert!(resolved.is_absolute());
    assert_eq!(resolved, nested.canonicalize().expect("canonical"));
}

/// Spaces are valid path characters, including at either end of a checkout's
/// name; the workspace grant must preserve them rather than treating `cwd` as
/// user-facing prose.
#[test]
fn a_working_directory_with_edge_whitespace_is_preserved() {
    let dir = tempfile::tempdir().expect("tempdir");
    let checkout = dir.path().join(" checkout ");
    std::fs::create_dir(&checkout).expect("mkdir");

    assert_eq!(
        turn_workspace_root(checkout.to_str().expect("utf-8 path")),
        Some(checkout.canonicalize().expect("canonical")),
    );
}

/// With nothing in the environment the turn asks for whatever the dispatch
/// already resolved — the node's model, the preset's, or the host default,
/// which by this point are one value.
#[test]
fn the_resolved_model_is_used_when_the_environment_names_none() {
    assert_eq!(
        super::effective_model(Some("deepseek/deepseek-v4-pro".into()), &HashMap::new()).as_deref(),
        Some("deepseek/deepseek-v4-pro")
    );
    assert!(super::effective_model(None, &HashMap::new()).is_none());
}

/// The operator's environment override outranks it: this is the knob that
/// answers "run this machine's turns on that model" without editing a config
/// file or a graph.
#[test]
fn the_environment_override_outranks_the_resolved_model() {
    let env: HashMap<String, String> = [(
        "MEDULLA_OPENHUMAN_MODEL".to_string(),
        "openrouter/chosen".to_string(),
    )]
    .into_iter()
    .collect();
    assert_eq!(
        super::effective_model(Some("preset/model".into()), &env).as_deref(),
        Some("openrouter/chosen")
    );

    // The generic key applies too, one tier lower.
    let env: HashMap<String, String> = [(
        "MEDULLA_HARNESS_MODEL".to_string(),
        "generic/model".to_string(),
    )]
    .into_iter()
    .collect();
    assert_eq!(
        super::effective_model(Some("preset/model".into()), &env).as_deref(),
        Some("generic/model")
    );
}

/// A blank on either side falls through rather than asking the core for the
/// empty model: `--model ""` and an exported-but-empty variable are both ways
/// of saying "no preference".
#[test]
fn blank_choices_fall_through_to_the_next_route() {
    let env: HashMap<String, String> = [("MEDULLA_OPENHUMAN_MODEL".to_string(), "  ".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        super::effective_model(Some("  preset/model  ".into()), &env).as_deref(),
        Some("preset/model")
    );
    assert!(super::effective_model(Some("   ".into()), &env).is_none());
}
