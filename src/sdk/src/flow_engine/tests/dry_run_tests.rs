//! Dry-run simulation: a graph validated by running it, with nothing dispatched.
//!
//! What remains of the old `http_tests` module after the HTTP capsule and the
//! schema sampler moved to `tinyflows::caps::host`. This case is not about
//! either: it is about the bundle Medulla assembles for a simulation, and the
//! claim that a validation run costs no harness session.

use serde_json::json;

use super::{agent_graph, empty_resolver};

#[tokio::test]
async fn a_dry_run_starts_no_harness_session_but_still_satisfies_declared_schemas() {
    let root = tempfile::tempdir().unwrap();
    let caps = super::super::build_dry_run_capabilities(empty_resolver(root.path()));

    let compiled = tinyflows::compiler::compile(&agent_graph(json!({
        "prompt": "summarise",
        "agent_ref": "builder",
        "output_parser": { "schema": { "type": "object", "properties": {
            "summary": { "type": "string" }
        }}}
    })))
    .unwrap();
    let outcome = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("a dry run of a valid graph must succeed");

    // `items[0].json` is the engine's `{ json, text, raw }` envelope, whose
    // `json` is the mock's own response object — hence the parsed payload sits
    // one level further in, at `.json.json`.
    assert!(
        outcome.output["nodes"]["step"]["items"][0]["json"]["json"]["json"]["summary"].is_string(),
        "the declared schema should be satisfied: {}",
        outcome.output
    );
}
