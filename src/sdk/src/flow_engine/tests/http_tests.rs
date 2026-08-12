//! HTTP capability behaviour: allowlist and SSRF guards, credential handling,
//! schema sampling, and dry-run simulation.
//!
//! Split out of [`super`] (see that module's doc comment) when the
//! network-capsule cases pushed the file over the repository's 500-line ceiling.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use tinyflows::caps::HttpClient;

use super::super::caps::http::{
    http_cred_name, inject_credential, is_private_addr, is_private_host, redacted_summary,
    vet_resolution, AllowlistHttpClient, HttpCredential,
};
use super::super::caps::mocks::sample_for_schema;
use super::super::settings::CapabilitySettings;
use super::{agent_graph, empty_resolver};

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

#[test]
fn private_address_detection_covers_the_cloud_metadata_endpoint() {
    // The one an SSRF is usually aiming for, and the reason link-local is
    // refused rather than only loopback.
    let metadata: std::net::IpAddr = "169.254.169.254".parse().unwrap();
    assert!(is_private_addr(&metadata));

    for private in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1", "::1"] {
        let addr: std::net::IpAddr = private.parse().unwrap();
        assert!(is_private_addr(&addr), "{private}");
    }
    for public in ["8.8.8.8", "1.1.1.1"] {
        let addr: std::net::IpAddr = public.parse().unwrap();
        assert!(!is_private_addr(&addr), "{public}");
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
fn vetting_returns_the_very_addresses_the_request_will_be_pinned_to() {
    // The vetted list is the point: it is handed to the transport as a DNS
    // override, so the answer checked here is the answer connected to and a
    // second lookup cannot rebind the name to something private in between.
    let refused = vet_resolution("localhost", 80).expect_err("loopback must not be vetted");
    assert!(
        refused.to_string().contains("loopback or private"),
        "{refused}"
    );

    // An IP literal resolves to itself, so a public one vets to exactly one
    // address and pins the transport to it.
    let vetted = vet_resolution("93.184.216.34", 443).expect("a public literal");
    assert_eq!(
        vetted,
        vec!["93.184.216.34:443".parse::<std::net::SocketAddr>().unwrap()]
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
        assert!(is_private_addr(&addr), "{mapped}");
    }
    // Unique-local fc00::/7 — the v6 answer to RFC 1918.
    let ula: std::net::IpAddr = "fd00::1".parse().unwrap();
    assert!(is_private_addr(&ula));

    let public: std::net::IpAddr = "::ffff:8.8.8.8".parse().unwrap();
    assert!(!is_private_addr(&public));
}
