//! Tool and code capability behaviour: the refused code runner, the allowlist
//! gate, and the preflight invoker that catches broken argument bindings.
//!
//! Split out of [`super`] (see that module's doc comment) when the capability
//! cases pushed the file over the repository's 500-line ceiling.

use std::sync::Arc;

use serde_json::{json, Value};
use tinyflows::caps::{CodeLanguage, CodeRunner, ToolInvoker};

use super::super::caps::code::DeniedCodeRunner;
use super::super::caps::tools::{MedullaToolInvoker, PreflightToolInvoker};
use super::super::settings::CapabilitySettings;
use super::settings;

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
