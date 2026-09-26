//! Live copilot turns against a real coding harness.
//!
//! Everything else in this suite is offline and deterministic, which is what
//! makes it runnable in CI — and also what makes it structurally unable to
//! answer the one question that matters most about the copilot: *does the
//! harness on the other end actually receive its `workflow_*` tools?* Every
//! mocked test stands a stub in for the harness, and a stub that writes to the
//! store is indistinguishable from a real agent whose tools never arrived.
//!
//! That failure mode is the reason this file exists. It does not look like a
//! failure: the session starts, the model answers confidently, and the graph is
//! unchanged. An operator reads a reply saying the workflow cannot be edited
//! and concludes the feature is broken, which is the report that prompted this.
//!
//! # Running it
//!
//! Opt in, because these turns start real harness sessions, cost real tokens,
//! and take minutes:
//!
//! ```text
//! MEDULLA_LIVE_COPILOT=1 cargo test -p medulla --test live_copilot -- --ignored --test-threads=1
//! ```
//!
//! Requirements: a coding-agent CLI on `PATH` (Claude Code today) and whatever
//! credentials it needs, plus a built `medulla` binary — `cargo build` first.
//! Each test runs under its own `MEDULLA_HOME`, so nothing here touches the
//! operator's own workflows.
//!
//! The binary matters and is not a detail. The tool server a harness is handed
//! is *this program* run as `medulla mcp`, and in a test harness "this program"
//! is `target/debug/deps/live_copilot-<hash>`, which serves nothing. Left
//! alone, every test below would fail with the agent reporting that it has no
//! workflow tools — which is a true statement about the test binary and says
//! nothing about the product. `MEDULLA_MCP_COMMAND` is the seam that points it
//! at the real one; [`medulla_binary`] finds it beside this test.
//!
//! `--test-threads=1` is not optional: [`LocalWorkflowHost`] binds fixed
//! loopback addresses, so two of these in parallel fight over them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use medulla::config::WorkflowsConfig;
use medulla::workflows::WorkflowStore;

/// Whether the operator asked for live turns.
///
/// Checked inside each test rather than only through `#[ignore]` so that
/// `--ignored` on a machine that has not opted in *skips* loudly instead of
/// starting a harness session nobody asked to pay for.
fn opted_in() -> bool {
    match std::env::var("MEDULLA_LIVE_COPILOT") {
        Ok(value) if !value.is_empty() && value != "0" => {}
        _ => {
            eprintln!("skipped: set MEDULLA_LIVE_COPILOT=1 to run live copilot turns");
            return false;
        }
    }
    // Set rather than merely checked, because the whole suite depends on it and
    // an operator should not have to know to export it by hand.
    match medulla_binary() {
        Some(path) => {
            std::env::set_var(medulla::mcp::SERVER_COMMAND_ENV, path);
            true
        }
        None => panic!(
            "no `medulla` binary beside this test — run `cargo build` first. Without it the \
             tool server is this test harness, which serves nothing, and every assertion \
             below would blame the product for it."
        ),
    }
}

/// The built `medulla` binary, beside this test's own executable.
///
/// `cargo` puts integration tests in `target/<profile>/deps/`, so the binary is
/// one directory up. Returning `None` rather than guessing at a path keeps the
/// failure legible: "build it first" is a better message than a harness turn
/// that reports having no tools.
fn medulla_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.parent()?.join("medulla");
    candidate.is_file().then_some(candidate)
}

/// An isolated Medulla home, and the store a turn's tools will write through.
///
/// The store is *discovered* rather than constructed from explicit directories,
/// and that is load-bearing. The agent's edits do not go through the handle
/// returned here — they go through the MCP subprocess, which discovers its own
/// store from `MEDULLA_HOME`. A hand-built store over `<home>/workflows` reads a
/// directory the subprocess never writes to (discovery adds an account segment),
/// so every assertion would see an empty catalogue and blame the agent for it.
fn scratch() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let home = tempfile::tempdir().expect("tempdir");
    std::env::set_var("MEDULLA_HOME", home.path());
    let env: HashMap<String, String> = std::env::vars().collect();
    let store = medulla::workflows::ops::discover_store(&env, home.path());
    (home, store)
}

/// The workflows config a live turn runs under.
///
/// Deliberately the defaults: this is testing what an operator gets out of the
/// box, and a config that enabled something the shipped default does not would
/// prove the wrong thing.
fn config() -> WorkflowsConfig {
    WorkflowsConfig::default()
}

/// Run one authoring turn, printing progress so a watched run is not silent.
async fn author(
    store: &Arc<dyn WorkflowStore>,
    cwd: &Path,
    target: Option<&str>,
    instruction: &str,
) -> medulla::workflows::CopilotOutcome {
    let (status, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            eprintln!("  · {}", line.replace('\u{1f}', " #"));
        }
    });
    let outcome = medulla::workflows::local::author_here(
        store.clone(),
        &config(),
        &medulla::harness_hooks::LaunchPolicy::default(),
        cwd,
        target,
        instruction,
        Some(status),
    )
    .await
    .expect("the turn ran");
    let _ = printer.await;
    outcome
}

/// The tools a turn must be able to *see*.
///
/// Named individually rather than counted: a count passes when the surface has
/// silently swapped one tool for another, which is exactly the drift this is
/// here to catch.
const REQUIRED_TOOLS: [&str; 5] = [
    "workflow_list",
    "workflow_get",
    "workflow_create",
    "workflow_apply_ops",
    "workflow_dry_run",
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts a real harness session; needs MEDULLA_LIVE_COPILOT=1"]
async fn a_copilot_turn_is_served_the_workflow_tools() {
    if !opted_in() {
        return;
    }
    let (home, store) = scratch();

    let outcome = author(
        &store,
        home.path(),
        None,
        "Do not create or change anything. Call `workflow_list` once, then reply with the \
         names of every tool you were given this turn, separated by spaces.",
    )
    .await;

    for tool in REQUIRED_TOOLS {
        assert!(
            outcome.reply.contains(tool),
            "the turn did not report `{tool}` among its tools — the copilot is a chatbot \
             that cannot edit a graph. Reply was:\n{}",
            outcome.reply
        );
    }
    assert!(
        outcome.changes.is_empty(),
        "a turn told to change nothing changed something: {:?}",
        outcome.changes
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts a real harness session; needs MEDULLA_LIVE_COPILOT=1"]
async fn a_create_turn_installs_a_workflow_the_store_can_read_back() {
    if !opted_in() {
        return;
    }
    let (home, store) = scratch();

    let outcome = author(
        &store,
        home.path(),
        None,
        "Build the smallest possible workflow: a manual trigger and one agent step that \
         says hello. Call it `greeter`.",
    )
    .await;

    let created = outcome
        .created
        .as_deref()
        .unwrap_or_else(|| panic!("no workflow was created. Reply was:\n{}", outcome.reply));
    // Asserted against the store, never the reply: "I created it" and "it is
    // there" are different claims, and only the second one is checkable.
    let record = store
        .get(created)
        .expect("the store is readable")
        .unwrap_or_else(|| panic!("`{created}` was reported created but is not in the store"));
    assert!(
        record
            .graph
            .nodes
            .iter()
            .any(|node| matches!(node.kind, tinyflows::model::NodeKind::Trigger)),
        "a workflow with no trigger cannot run"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts a real harness session; needs MEDULLA_LIVE_COPILOT=1"]
async fn a_revise_turn_changes_the_stored_graph() {
    if !opted_in() {
        return;
    }
    let (home, store) = scratch();

    let created = author(
        &store,
        home.path(),
        None,
        "Build the smallest possible workflow: a manual trigger and one agent step. Call \
         it `greeter`.",
    )
    .await
    .created
    .expect("a workflow to revise");

    let before = store
        .get(&created)
        .expect("readable")
        .expect("present")
        .graph
        .nodes
        .len();

    let outcome = author(
        &store,
        home.path(),
        Some(&created),
        "Add one more agent step after the existing one that summarises what the first \
         step did.",
    )
    .await;

    let after = store
        .get(&created)
        .expect("readable")
        .expect("present")
        .graph
        .nodes
        .len();
    assert!(
        after > before,
        "the graph did not grow — a revise turn that reports success and changes nothing \
         is the failure this suite exists to catch. Reply was:\n{}",
        outcome.reply
    );
    assert!(
        !outcome.changes.is_empty(),
        "the change list is derived from the store, so it should agree with the graph"
    );
}

/// The tool surface, checked without spending a harness session.
///
/// Not `#[ignore]`d and not gated: it starts the MCP server this binary would
/// hand a harness and asks it what it serves, which is offline, fast, and
/// deterministic. It cannot prove the harness *received* the tools — that is
/// what the ignored tests above are for — but it does catch the far more
/// common regression of the server itself withholding them.
#[tokio::test]
async fn the_tool_server_a_copilot_session_gets_serves_the_authoring_surface() {
    let home = tempfile::tempdir().expect("tempdir");
    let env: HashMap<String, String> = [(
        "MEDULLA_HOME".to_string(),
        home.path().to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect();

    let served = medulla::mcp::tool_definitions(&medulla::mcp::McpSession::local(
        medulla::workflows::ops::discover_store(&env, home.path()),
        Default::default(),
        Default::default(),
    ));
    let names: Vec<&str> = served
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
        .collect();

    for tool in REQUIRED_TOOLS {
        assert!(
            names.contains(&tool),
            "the default session is not served `{tool}`; it has {names:?}"
        );
    }
}
