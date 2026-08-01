//! The `workflow_host` and `workflow_defaults` tools: what a workflow author
//! needs to know before naming a harness, model, or custom preset, and the
//! tool that lets them pin or clear a workflow's own `defaults` block.
//!
//! Split out of `cases.rs` to stay under this repository's 500-line ceiling;
//! shared fixtures (`store`, `document`, `config`, `call`) reach here through
//! `super::*`, the same way `propose.rs` reaches them.

use super::*;

#[tokio::test]
async fn the_host_tool_reports_what_this_machine_permits() {
    let (_root, store) = store();

    let (facts, is_error) = call(&store, "workflow_host", json!({})).await;

    assert!(!is_error, "{facts}");
    // The native slugs are the ones always available; an author guessing at
    // them is how a graph saves cleanly and fails at run time.
    assert!(facts["nativeTools"]
        .as_array()
        .unwrap()
        .contains(&json!("medulla:echo")));
    assert!(facts["allowCode"].is_boolean());
    assert!(
        facts["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("manual")),
        "the trigger limitation has to be stated: {facts}"
    );
}

#[tokio::test]
async fn the_host_tool_states_whether_shell_scripts_are_available() {
    // An author needs this before writing a `medulla:shell` step, not after it
    // fails on whichever host actually runs the workflow: `run_script` refuses
    // `language: shell` on Windows rather than emulating a POSIX shell there.
    let (_root, store) = store();

    let (facts, _) = call(&store, "workflow_host", json!({})).await;

    assert_eq!(facts["shellScriptsAvailable"], json!(!cfg!(windows)));
    let notes: Vec<&str> = facts["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap())
        .collect();
    if cfg!(windows) {
        assert!(
            notes.iter().any(|note| note.contains("javascript")),
            "{notes:?}"
        );
    } else {
        assert!(
            notes.iter().any(|note| note.contains("language: shell")),
            "{notes:?}"
        );
    }
}

#[tokio::test]
async fn the_host_tool_says_plainly_when_there_is_no_default_worker() {
    let (_root, store) = store();

    let (facts, _) = call(&store, "workflow_host", json!({})).await;

    // Default config configures none, so every agent node must name one — and
    // a node that does not fails at run time rather than at save time.
    assert_eq!(facts["defaultWorker"], json!(null));
    assert!(
        facts["notes"][0]
            .as_str()
            .unwrap()
            .contains("must name a `config.agent_ref`"),
        "{facts}"
    );
}

#[tokio::test]
async fn the_defaults_tool_pins_and_then_clears_a_workflow_s_harness() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    let (set, _) = call(
        &store,
        "workflow_defaults",
        json!({ "id": "sweep", "harness": "codex", "model": "gpt-5-codex" }),
    )
    .await;
    assert_eq!(set["defaults"]["harness"], "codex");

    // Read back through the ordinary fetch, not from the write's own claim.
    let (fetched, _) = call(&store, "workflow_get", json!({ "id": "sweep" })).await;
    assert_eq!(fetched["defaults"]["model"], "gpt-5-codex");

    // An empty string clears one field and leaves the other alone.
    let (cleared, _) = call(
        &store,
        "workflow_defaults",
        json!({ "id": "sweep", "harness": "" }),
    )
    .await;
    assert!(cleared["defaults"].get("harness").is_none(), "{cleared}");
    assert_eq!(cleared["defaults"]["model"], "gpt-5-codex");
}

#[tokio::test]
async fn the_defaults_tool_refuses_a_harness_it_cannot_read() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    let (_, is_error) = call(
        &store,
        "workflow_defaults",
        json!({ "id": "sweep", "harness": "claude code" }),
    )
    .await;

    assert!(is_error, "an unreadable harness must be refused");
    let (fetched, _) = call(&store, "workflow_get", json!({ "id": "sweep" })).await;
    assert!(
        fetched["defaults"].get("harness").is_none(),
        "the workflow must be left untouched: {fetched}"
    );
}

#[tokio::test]
async fn the_host_tool_lists_the_harnesses_a_node_may_choose_between() {
    let (_root, store) = store();

    let (facts, _) = call(&store, "workflow_host", json!({})).await;

    assert_eq!(
        facts["builtinHarnesses"],
        json!(["claude", "codex", "opencode"])
    );
    // Default config pins neither, so a node inherits whatever the worker runs.
    assert_eq!(facts["defaultHarness"], json!(null));
    assert_eq!(facts["defaultModel"], json!(null));
    // No presets configured in the fixture, but the key is always present so an
    // author can tell "none configured" from "this host does not say".
    assert_eq!(facts["customHarnesses"], json!([]));
    let notes: Vec<&str> = facts["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|note| note.as_str())
        .collect();
    assert!(
        notes.iter().any(|note| note.contains("config.harness")),
        "{notes:?}"
    );
}

#[tokio::test]
async fn the_host_tool_names_the_custom_presets_this_machine_has() {
    let (_root, store) = store();
    let policy = crate::workflows::ops::HostPolicy {
        custom_harnesses: vec!["deepseek-claude".into(), "kimi-codex".into()],
        ..Default::default()
    };
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_host", "arguments": {} },
    });

    let response = handle_request(&store, &policy, ToolMode::Full, &request)
        .await
        .expect("a response");
    let facts: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();

    assert_eq!(
        facts["customHarnesses"],
        json!(["deepseek-claude", "kimi-codex"])
    );
}
