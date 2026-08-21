//! Unit tests for the embedded-core provider's routing and reply decoding.
//!
//! Deliberately no test that actually runs a turn: doing so would boot the
//! process-wide core, which starts background services and binds a workspace —
//! see [`crate::core_host::shared`] on why exactly one of those may exist per
//! process. What *is* testable here is every decision made around the call.

use std::collections::HashMap;

use crate::protocol::{HarnessProvider, HarnessTransport};
use crate::sessions::SessionClass;

use super::super::types::{Abort, RunTaskOptions};
use super::run::uses_embedded_core;

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

// The `RpcOutcome` envelope tests that used to live here are gone with
// `reply_text`. Whether a controller wraps its answer in `{result, logs}` is
// upstream's implementation detail, and this host no longer decodes it: the
// typed facade returns a `TurnOutcome` whose `reply` is already the answer, and
// the unwrapping is tested once, next to the code that does it
// (`openhuman_core::embed::call`). Two hosts keeping private copies of that
// heuristic — this one had a looser variant than `core_host::auth`'s — is the
// duplication the port removed.

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
