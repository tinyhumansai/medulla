//! Tests for the capability seam.
//!
//! Every test is offline and deterministic. The one that matters most drives a
//! real graph through the real engine into the real agent adapter, and asserts
//! on the task frame that came out the other side — that is the claim "a
//! workflow step is a harness session" reduced to something checkable.
//!
//! The `medulla:shell` tool's own cases live in the sibling `shell_tests`
//! module, split out once they pushed this file over the 500-line ceiling.
//! `settings` below is `pub(super)` so that module can reuse it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tinyflows::caps::{CodeLanguage, CodeRunner, HttpClient, StateStore, ToolInvoker};
use tinyflows::model::WorkflowGraph;

use super::caps::agent::{instruction_of, reply_to_value, route_for_agent_ref, AgentRoute};
use super::caps::code::DeniedCodeRunner;
use super::caps::dispatch::HarnessDispatch;
use super::caps::http::{
    http_cred_name, inject_credential, is_private_host, redacted_summary, AllowlistHttpClient,
    HttpCredential,
};
use super::caps::mocks::sample_for_schema;
use super::caps::state::FileStateStore;
use super::caps::tools::{MedullaToolInvoker, PreflightToolInvoker};
use super::caps::{build_capabilities, HostServices};
use super::settings::CapabilitySettings;
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::StoreWorkflowResolver;

/// A dispatch that records what it was asked to run and replies with a fixed
/// answer, so a test can assert on the frame rather than on a live worker.
#[derive(Default)]
struct RecordingDispatch {
    seen: Mutex<Vec<TaskRequest>>,
    reply: String,
    fail: Option<RunError>,
}

impl RecordingDispatch {
    fn replying(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: reply.to_string(),
            fail: None,
        })
    }

    fn failing(err: RunError) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: String::new(),
            fail: Some(err),
        })
    }

    fn requests(&self) -> Vec<TaskRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl HarnessDispatch for RecordingDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        match &self.fail {
            Some(err) => Err(err.clone()),
            None => Ok(TaskOutcome {
                reply: self.reply.clone(),
                usage: crate::tinyplace::TokenUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
                harness: None,
            }),
        }
    }
}

/// A resolver over an empty store: no test here uses `sub_workflow`, but the
/// engine requires the capability to be present.
fn empty_resolver(root: &std::path::Path) -> Arc<StoreWorkflowResolver> {
    Arc::new(StoreWorkflowResolver::new(Arc::new(
        crate::workflows::FileWorkflowStore::new(vec![root.join("workflows")], root.join("runs")),
    )))
}

pub(super) fn settings(root: &std::path::Path) -> Arc<CapabilitySettings> {
    let mut settings = CapabilitySettings::rooted_at(root);
    settings.default_worker_address = "local-worker".into();
    Arc::new(settings)
}

/// A one-agent-node graph: trigger into an agent, which is the smallest thing
/// that exercises the dispatch path end to end.
fn agent_graph(config: Value) -> WorkflowGraph {
    serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "step", "kind": "agent", "name": "step", "config": config }
        ],
        "edges": [{ "from_node": "t", "to_node": "step" }]
    }))
    .unwrap()
}

#[tokio::test]
async fn an_agent_node_becomes_a_task_frame_dispatched_to_a_harness() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("done: refactored the parser");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-7",
    );

    let compiled = tinyflows::compiler::compile(&agent_graph(json!({
        "prompt": "refactor the parser",
        "agent_ref": "builder"
    })))
    .expect("compiles");
    let outcome = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    let requests = dispatch.requests();
    assert_eq!(requests.len(), 1, "one node, one dispatch");
    let request = &requests[0];
    assert_eq!(request.instruction, "refactor the parser");
    assert_eq!(
        request.worker_address, "builder",
        "the agent_ref selects the worker"
    );
    assert_eq!(
        request.abort_id, "run-7",
        "aborting the run must cancel the node in flight"
    );

    // The reply reaches the node's output through the engine's envelope, which
    // is what makes `=item.text` resolve for a downstream node.
    assert_eq!(
        outcome.output["nodes"]["step"]["items"][0]["json"]["text"],
        "done: refactored the parser"
    );
}

#[tokio::test]
async fn a_node_without_an_agent_ref_runs_on_the_configured_default_worker() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("ok");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-1",
    );

    let compiled =
        tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "hello" }))).unwrap();
    tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    assert_eq!(dispatch.requests()[0].worker_address, "local-worker");
}

#[tokio::test]
async fn a_worker_failure_surfaces_as_a_capability_error_naming_the_cause() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::failing(RunError::Worker("clone failed".into()));
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            dispatch,
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-2",
    );

    let compiled =
        tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "build" }))).unwrap();
    let err = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect_err("the node must fail");

    assert!(
        err.to_string().contains("clone failed"),
        "the worker's own message should survive: {err}"
    );
}

#[tokio::test]
async fn dispatching_with_no_worker_configured_explains_what_to_set() {
    let root = tempfile::tempdir().unwrap();
    let mut bare = CapabilitySettings::rooted_at(root.path());
    bare.default_worker_address = String::new();
    let caps = build_capabilities(
        Arc::new(bare),
        HostServices {
            dispatch: RecordingDispatch::replying("unused"),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-3",
    );

    let compiled = tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "hi" }))).unwrap();
    let err = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect_err("nowhere to send it");

    assert!(
        err.to_string().contains("agent_ref"),
        "the message should say how to fix it: {err}"
    );
}

#[test]
fn an_empty_agent_ref_routes_to_the_default_rather_than_a_worker_named_nothing() {
    assert_eq!(route_for_agent_ref(None), AgentRoute::Default);
    assert_eq!(route_for_agent_ref(Some("   ")), AgentRoute::Default);
    assert_eq!(
        route_for_agent_ref(Some("builder")),
        AgentRoute::Template("builder".into())
    );
}

#[test]
fn a_node_may_name_its_instruction_prompt_or_instruction() {
    assert_eq!(
        instruction_of(&json!({ "prompt": "a" })).unwrap(),
        "a".to_string()
    );
    assert_eq!(
        instruction_of(&json!({ "instruction": "b" })).unwrap(),
        "b".to_string()
    );
    let err = instruction_of(&json!({ "prompt": "  " })).expect_err("blank is not an instruction");
    assert!(err.to_string().contains("prompt"), "got {err}");
}

#[test]
fn a_json_reply_is_surfaced_structurally_as_well_as_textually() {
    let value = reply_to_value("{\"files\": 3}", "builder");
    assert_eq!(value["json"]["files"], 3);
    assert_eq!(value["text"], "{\"files\": 3}");

    // Prose is still fine; it just has no structure to offer.
    let prose = reply_to_value("all done", "builder");
    assert!(prose["json"].is_null());
    assert_eq!(prose["text"], "all done");
}

#[tokio::test]
async fn state_is_scoped_per_namespace_so_two_workflows_cannot_collide() {
    let root = tempfile::tempdir().unwrap();
    let alpha = FileStateStore::new(root.path(), "workflow:alpha");
    let beta = FileStateStore::new(root.path(), "workflow:beta");

    alpha.store("cursor", json!(1)).await.unwrap();
    beta.store("cursor", json!(2)).await.unwrap();

    assert_eq!(alpha.load("cursor").await.unwrap(), Some(json!(1)));
    assert_eq!(beta.load("cursor").await.unwrap(), Some(json!(2)));
    assert_eq!(alpha.load("missing").await.unwrap(), None);
}

#[tokio::test]
async fn a_state_key_containing_path_separators_cannot_escape_its_directory() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStateStore::new(&root.path().join("state"), "workflow:alpha");

    store.store("../../escaped", json!("x")).await.unwrap();

    // Everything written must live under the namespace directory.
    let escaped = root.path().join("escaped.json");
    assert!(!escaped.exists(), "a key must not choose its own path");
    assert_eq!(
        store.load("../../escaped").await.unwrap(),
        Some(json!("x")),
        "and it must still round-trip"
    );
}

#[tokio::test]
async fn code_nodes_are_refused_by_default_with_a_reason() {
    let err = DeniedCodeRunner
        .run(CodeLanguage::JavaScript, "console.log(1)", json!([]))
        .await
        .expect_err("must refuse");

    assert!(
        err.to_string().contains("sandbox"),
        "the refusal should say why: {err}"
    );
}

#[tokio::test]
async fn native_tools_run_and_unlisted_third_party_tools_do_not() {
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(settings(root.path()));

    let echoed = invoker
        .invoke("medulla:echo", json!({ "a": 1 }), None)
        .await
        .expect("native tools need no allowlist");
    assert_eq!(echoed["echo"]["a"], 1);

    let err = invoker
        .invoke("github.create_issue", json!({}), None)
        .await
        .expect_err("deny by default");
    assert!(err.to_string().contains("allowlist"), "got {err}");

    let unknown = invoker
        .invoke("medulla:nope", json!({}), None)
        .await
        .expect_err("unknown native tool");
    assert!(
        unknown.to_string().contains("medulla:echo"),
        "the error should list what does exist: {unknown}"
    );
}

#[tokio::test]
async fn a_listed_tool_this_host_cannot_run_says_so_rather_than_blaming_the_allowlist() {
    let root = tempfile::tempdir().unwrap();
    let mut with_allow = CapabilitySettings::rooted_at(root.path());
    with_allow.tool_allowlist = vec!["github.create_issue".into()];

    let err = MedullaToolInvoker::new(Arc::new(with_allow))
        .invoke("github.create_issue", json!({}), None)
        .await
        .expect_err("nothing can run it");

    assert!(
        err.to_string().contains("no integration registry"),
        "the operator did their part; say what is actually missing: {err}"
    );
}

#[tokio::test]
async fn preflight_catches_an_argument_whose_expression_never_resolved() {
    let inner = tinyflows::caps::mock::mock_capabilities().tools;
    let preflight = PreflightToolInvoker::new(inner);

    let err = preflight
        .invoke("anything", json!({ "issue": Value::Null }), None)
        .await
        .expect_err("a null argument is a broken binding");

    assert!(err.to_string().contains("issue"), "name the field: {err}");
}

#[tokio::test]
async fn http_refuses_loopback_and_anything_off_the_allowlist() {
    let root = tempfile::tempdir().unwrap();
    let mut allowed = CapabilitySettings::rooted_at(root.path());
    allowed.http_allowlist = vec!["example.com".into()];
    let client = AllowlistHttpClient::new(Arc::new(allowed), HashMap::new());

    // Loopback is refused even though the test could otherwise serve it — a
    // workflow reaching localhost is reaching services that trusted the network
    // boundary.
    let loopback = client
        .request(json!({ "url": "http://127.0.0.1:8080/x" }), None)
        .await
        .expect_err("loopback");
    assert!(loopback.to_string().contains("private"), "got {loopback}");

    let off_list = client
        .request(json!({ "url": "https://elsewhere.test/x" }), None)
        .await
        .expect_err("not allowlisted");
    assert!(off_list.to_string().contains("allowlist"), "got {off_list}");
}

#[test]
fn private_host_detection_covers_loopback_names_and_ranges_but_not_lookalikes() {
    for private in [
        "localhost",
        "127.0.0.1",
        "10.1.2.3",
        "192.168.0.1",
        "169.254.1.1",
        "::1",
        "db.internal",
    ] {
        assert!(is_private_host(private), "{private} should be refused");
    }
    for public in ["example.com", "notlocalhost.com", "8.8.8.8"] {
        assert!(!is_private_host(public), "{public} should be reachable");
    }
}

#[test]
fn an_unrecognised_connection_ref_fails_closed_rather_than_sending_unauthenticated() {
    assert_eq!(http_cred_name(None).unwrap(), None);
    assert_eq!(http_cred_name(Some("http_cred:ci")).unwrap(), Some("ci"));

    // Silently dropping it would send the request anyway, without the
    // credential the author asked for.
    assert!(http_cred_name(Some("composio:abc")).is_err());
    assert!(http_cred_name(Some("http_cred:")).is_err());
}

#[test]
fn a_credential_is_injected_after_the_summary_is_taken() {
    let request = json!({ "method": "post", "url": "https://example.com/x" });
    let summary = redacted_summary(&request);
    let sent = inject_credential(
        request,
        &HttpCredential {
            header: "Authorization".into(),
            value: "Bearer super-secret".into(),
        },
    );

    assert_eq!(summary, "POST https://example.com/x");
    assert!(
        !summary.contains("super-secret"),
        "a secret must never reach a log or an approval prompt"
    );
    assert_eq!(sent["headers"]["Authorization"], "Bearer super-secret");
}

#[test]
fn a_dry_run_sample_satisfies_the_shape_a_node_declared() {
    let sample = sample_for_schema(&json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "count": { "type": "integer" },
            "tags":  { "type": "array", "items": { "type": "string" } },
            "state": { "enum": ["open", "closed"] }
        }
    }));

    assert!(sample["title"].is_string());
    assert!(sample["count"].is_number());
    // One element, so a downstream per-item node has something to map over.
    assert_eq!(sample["tags"].as_array().unwrap().len(), 1);
    assert_eq!(sample["state"], "open");
}

#[tokio::test]
async fn a_dry_run_starts_no_harness_session_but_still_satisfies_declared_schemas() {
    let root = tempfile::tempdir().unwrap();
    let caps = super::build_dry_run_capabilities(empty_resolver(root.path()));

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

#[test]
fn private_address_detection_covers_the_cloud_metadata_endpoint() {
    // The one an SSRF is usually aiming for, and the reason link-local is
    // refused rather than only loopback.
    let metadata: std::net::IpAddr = "169.254.169.254".parse().unwrap();
    assert!(super::caps::http::is_private_addr(&metadata));

    for private in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1", "::1"] {
        let addr: std::net::IpAddr = private.parse().unwrap();
        assert!(super::caps::http::is_private_addr(&addr), "{private}");
    }
    for public in ["8.8.8.8", "1.1.1.1"] {
        let addr: std::net::IpAddr = public.parse().unwrap();
        assert!(!super::caps::http::is_private_addr(&addr), "{public}");
    }
}

#[tokio::test]
async fn an_allowlisted_name_that_resolves_to_loopback_is_still_refused() {
    // The textual guard cannot catch this: `localtest.me` and friends are
    // ordinary names whose DNS answer is 127.0.0.1. Resolving is what closes
    // the rebinding gap.
    let root = tempfile::tempdir().unwrap();
    let mut allowed = CapabilitySettings::rooted_at(root.path());
    allowed.http_allowlist = vec!["localtest.me".into()];
    let client = AllowlistHttpClient::new(Arc::new(allowed), HashMap::new());

    let result = client
        .request(json!({ "url": "http://localtest.me/x" }), None)
        .await;

    // Either it resolved to loopback and was refused for that, or this machine
    // has no DNS for the name and it was refused for that — never sent.
    let err = result.expect_err("must not be sent");
    let message = err.to_string();
    assert!(
        message.contains("loopback or private") || message.contains("cannot resolve"),
        "got {message}"
    );
}

#[test]
fn an_ipv4_mapped_ipv6_loopback_is_recognised_as_private() {
    // `::ffff:127.0.0.1` reaches loopback exactly as `127.0.0.1` does, so
    // judging it by the v6 rules alone would let it through.
    for mapped in [
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::ffff:169.254.169.254",
    ] {
        let addr: std::net::IpAddr = mapped.parse().unwrap();
        assert!(super::caps::http::is_private_addr(&addr), "{mapped}");
    }
    // Unique-local fc00::/7 — the v6 answer to RFC 1918.
    let ula: std::net::IpAddr = "fd00::1".parse().unwrap();
    assert!(super::caps::http::is_private_addr(&ula));

    let public: std::net::IpAddr = "::ffff:8.8.8.8".parse().unwrap();
    assert!(!super::caps::http::is_private_addr(&public));
}

#[tokio::test]
async fn two_nodes_on_one_worker_get_distinct_wire_ids() {
    // A worker dedupes on sender + taskId, so two parallel nodes sharing an id
    // would have one silently rejected as a duplicate of the other.
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("ok");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-dup",
    );

    let graph: WorkflowGraph = serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start" },
            { "id": "left", "kind": "agent", "name": "L", "config": { "prompt": "a" } },
            { "id": "right", "kind": "agent", "name": "R", "config": { "prompt": "b" } }
        ],
        "edges": [
            { "from_node": "t", "to_node": "left" },
            { "from_node": "t", "to_node": "right" }
        ]
    }))
    .unwrap();
    let compiled = tinyflows::compiler::compile(&graph).unwrap();
    tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    let ids: Vec<String> = dispatch
        .requests()
        .iter()
        .map(|r| r.task_id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "both nodes routed to the default worker: {ids:?}"
    );
}
