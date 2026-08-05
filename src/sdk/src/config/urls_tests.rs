//! Unit tests for endpoint base-URL resolution and display-host formatting.

use super::*;
use std::collections::HashMap;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn backend_url_precedence() {
    // Nothing set → prod default.
    assert_eq!(
        resolve_backend_base_url(&env(&[]), None),
        "https://api.tinyhumans.ai"
    );
    // Staging switch flips the default.
    assert_eq!(
        resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "1")]), None),
        "https://staging-api.tinyhumans.ai"
    );
    assert_eq!(
        resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "TRUE")]), None),
        "https://staging-api.tinyhumans.ai"
    );
    // A non-truthy value keeps prod.
    assert_eq!(
        resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "no")]), None),
        "https://api.tinyhumans.ai"
    );
    // Explicit config beats the (staging) default.
    assert_eq!(
        resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "1")]), Some("http://x:1")),
        "http://x:1"
    );
    // MEDULLA_API_URL beats both config and default.
    assert_eq!(
        resolve_backend_base_url(
            &env(&[
                ("MEDULLA_STAGING", "1"),
                ("MEDULLA_API_URL", "http://env:2")
            ]),
            Some("http://x:1")
        ),
        "http://env:2"
    );
    // An empty MEDULLA_API_URL is ignored; config wins.
    assert_eq!(
        resolve_backend_base_url(&env(&[("MEDULLA_API_URL", "")]), Some("http://x:1")),
        "http://x:1"
    );
}

#[test]
fn forwarder_url_follows_the_backend_unless_configured() {
    // The forwarder is served by the same backend as everything else, so it
    // derives from the already-resolved backend URL rather than naming a second
    // host. An operator who moves `backend.baseUrl` moves the forwarder with it,
    // which is the whole point: two endpoints on different forwarders both start
    // cleanly and never hear from each other.
    assert_eq!(
        resolve_forwarder_base_url("https://api.tinyhumans.ai", None),
        "https://api.tinyhumans.ai"
    );
    assert_eq!(
        resolve_forwarder_base_url("https://staging-api.tinyhumans.ai", None),
        "https://staging-api.tinyhumans.ai"
    );
    // Explicit `link.forwarderUrl` still wins — a split deployment is allowed,
    // it just has to be asked for.
    assert_eq!(
        resolve_forwarder_base_url("https://api.tinyhumans.ai", Some("https://cfg")),
        "https://cfg"
    );
    // Blank / whitespace config is not a configuration.
    assert_eq!(
        resolve_forwarder_base_url("https://api.tinyhumans.ai", Some("   ")),
        "https://api.tinyhumans.ai"
    );
}

#[test]
fn display_host_strips_scheme_port_and_path() {
    use super::display_host;
    assert_eq!(
        display_host("https://api.tinyhumans.ai"),
        "api.tinyhumans.ai"
    );
    assert_eq!(
        display_host("https://api.tinyhumans.ai/v1/chat?x=1#f"),
        "api.tinyhumans.ai"
    );
    assert_eq!(display_host("http://localhost:4000"), "localhost");
    assert_eq!(
        display_host("  https://staging-api.tinyhumans.ai/  "),
        "staging-api.tinyhumans.ai"
    );
    assert_eq!(
        display_host("https://user:pw@api.example.com/x"),
        "api.example.com"
    );
    assert_eq!(display_host("http://[::1]:8080/v1"), "[::1]");
}

#[test]
fn display_host_passes_through_unparseable_input() {
    use super::display_host;
    // Display-only: a malformed base URL is shown verbatim so the mistake is visible.
    assert_eq!(display_host("not a url"), "not a url");
    assert_eq!(display_host("api.tinyhumans.ai"), "api.tinyhumans.ai");
    assert_eq!(display_host("https://"), "https://");
}
