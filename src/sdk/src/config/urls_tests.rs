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
fn tinyplace_url_precedence() {
    assert_eq!(
        resolve_tinyplace_base_url(&env(&[]), None),
        "https://api.tiny.place"
    );
    assert_eq!(
        resolve_tinyplace_base_url(&env(&[("MEDULLA_STAGING", "true")]), None),
        "https://staging-api.tiny.place"
    );
    // Explicit config beats the staging default.
    assert_eq!(
        resolve_tinyplace_base_url(&env(&[("MEDULLA_STAGING", "1")]), Some("https://cfg")),
        "https://cfg"
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
        display_host("  https://staging-api.tiny.place/  "),
        "staging-api.tiny.place"
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
