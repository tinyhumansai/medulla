//! Unit tests for the local provider's routing and its pre-flight refusals.
//!
//! Deliberately no test that actually runs a turn: that needs a live inference
//! endpoint, which is the one thing these suites must not require. What *is*
//! testable here is every decision made around the call — which provider takes
//! this path, and the states the turn refuses before it dials anything.

use std::collections::HashMap;

use crate::protocol::{HarnessProvider, HarnessTransport};
use crate::sessions::SessionClass;

use super::super::types::{Abort, RunTaskOptions};
use super::run::uses_local_harness;

/// Options naming `provider`, with everything else at its least interesting.
fn options(provider: HarnessProvider) -> RunTaskOptions {
    RunTaskOptions {
        embed: Default::default(),
        budget: None,
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

/// One provider takes the in-process path; every CLI provider takes the spawn
/// path. This is the whole of the routing decision, and getting it wrong in
/// either direction is silent — a CLI routed here would never spawn, and this
/// one routed to the spawn seam would look for a binary that does not exist.
#[test]
fn only_the_local_provider_runs_in_process() {
    assert!(uses_local_harness(&options(HarnessProvider::Openhuman)));
    assert!(!uses_local_harness(&options(HarnessProvider::Claude)));
    assert!(!uses_local_harness(&options(HarnessProvider::Codex)));
    assert!(!uses_local_harness(&options(HarnessProvider::Opencode)));
}

/// An already-aborted task fails before anything is dialled. Asserted because
/// the alternative is spending a model call on work that was already cancelled.
#[tokio::test]
async fn an_aborted_task_fails_before_dialling() {
    let options = options(HarnessProvider::Openhuman);
    options.abort.abort();

    let error = super::run_local_task(options)
        .await
        .expect_err("an aborted task must not run");
    assert!(error.contains("aborted before start"), "{error}");
}

/// With no router preset there is no endpoint and no credential, and unlike a
/// CLI provider — whose child resolves its own — this harness has nothing to
/// fall back to. It must say so by name rather than failing on its first
/// request with a provider error about a missing key.
#[tokio::test]
async fn a_turn_with_no_route_says_what_is_missing() {
    let error = super::run_local_task(options(HarnessProvider::Openhuman))
        .await
        .expect_err("a turn with no route cannot run");
    assert!(error.contains("inference route"), "{error}");
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
