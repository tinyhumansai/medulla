//! Unit tests for OpenPanel configuration, payload shape, and transport.

use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::json;

use super::config::{resolve, OpenPanelConfig, DEFAULT_API_URL, DEFAULT_CLIENT_ID};
use super::payload::Payload;
use super::{AnalyticsError, AnalyticsStatus, Tracker};

fn defaults() -> OpenPanelConfig {
    OpenPanelConfig::medulla()
}

#[test]
fn identify_carries_only_the_account_id() {
    assert_eq!(
        serde_json::to_value(Payload::identify("user-42")).unwrap(),
        json!({ "type": "identify", "payload": { "profileId": "user-42" } })
    );
}

#[test]
fn track_matches_the_sdk_wire_shape() {
    let payload = Payload::track(
        "token_usage_reported",
        Some("user-42"),
        [
            ("input_tokens", "10".to_owned()),
            ("output_tokens", "3".to_owned()),
        ],
    );
    assert_eq!(
        serde_json::to_value(payload).unwrap(),
        json!({
            "type": "track",
            "payload": {
                "name": "token_usage_reported",
                "profileId": "user-42",
                "properties": {
                    "app": "medulla",
                    "version": env!("CARGO_PKG_VERSION"),
                    "input_tokens": "10",
                    "output_tokens": "3",
                }
            }
        })
    );
}

#[test]
fn an_anonymous_track_omits_the_profile_instead_of_sending_null() {
    let value = serde_json::to_value(Payload::track("analytics_test", None, [])).unwrap();
    assert!(value["payload"].get("profileId").is_none(), "{value}");
}

#[test]
fn the_opt_out_wins_over_everything() {
    assert_eq!(resolve(true), Err(AnalyticsStatus::Disabled));
}

#[test]
fn analytics_is_active_by_default_with_no_secret_required() {
    assert_eq!(resolve(false), Ok(defaults()));
}

#[tokio::test]
async fn this_crates_tests_never_reach_the_live_project() {
    assert_eq!(super::status(), AnalyticsStatus::Disabled);
    assert!(matches!(
        super::record_sign_in("user-42").await,
        Err(AnalyticsError::Inactive(AnalyticsStatus::Disabled))
    ));
    assert!(matches!(
        super::send_test_event().await,
        Err(AnalyticsError::Inactive(AnalyticsStatus::Disabled))
    ));
    // The fire-and-forget helpers are no-ops rather than panics.
    super::record_screen_view("Chat");
    super::record_ui_action("command_dispatched");
    super::record_token_usage(1, 2);
}

#[test]
fn every_build_reports_to_the_medulla_project() {
    let config = defaults();
    assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
    assert_eq!(config.client_id, "781d9ce2-62ec-4059-a093-152c88400576");
    assert_eq!(config.endpoint(), format!("{DEFAULT_API_URL}/track"));
    assert_eq!(config.endpoint(), "https://panel.tinyhumans.ai/api/track");
}

#[test]
fn a_trailing_slash_on_the_base_url_is_dropped() {
    let config = OpenPanelConfig::new("https://panel.example.test/api/", "client-1");
    assert_eq!(config.endpoint(), "https://panel.example.test/api/track");
}

#[test]
fn headers_carry_the_client_id_and_never_a_secret() {
    let headers = defaults().headers().expect("valid headers");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["openpanel-client-id"], DEFAULT_CLIENT_ID);
    assert_eq!(headers["openpanel-sdk-name"], "medulla");
    assert_eq!(headers["openpanel-sdk-version"], env!("CARGO_PKG_VERSION"));
    assert!(
        headers.keys().all(|name| !name.as_str().contains("secret")),
        "{headers:?}"
    );
}

#[test]
fn analytics_endpoint_requires_tls_except_for_loopback_diagnostics() {
    assert!(super::endpoint_is_allowed(
        "https://panel.example/api/track"
    ));
    assert!(super::endpoint_is_allowed(
        "http://127.0.0.1:3000/api/track"
    ));
    assert!(super::endpoint_is_allowed("http://[::1]:3000/api/track"));
    assert!(super::endpoint_is_allowed(
        "http://localhost:3000/api/track"
    ));
    assert!(!super::endpoint_is_allowed(
        "http://panel.example/api/track"
    ));
    assert!(!super::endpoint_is_allowed(
        "http://localhost.evil.example/api/track"
    ));
}

#[test]
fn an_illegal_header_value_disables_the_tracker_instead_of_panicking() {
    let config = OpenPanelConfig::new(DEFAULT_API_URL, "bad\nclient");
    assert!(config.headers().is_none());
    assert!(Tracker::new(&config).is_none());
}

/// Accept one HTTP request on a local listener, answer it with `status`, and
/// return the raw request text.
fn serve_once(status: u16) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/api", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).expect("read");
            request.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&request);
            if let Some(head_end) = text.find("\r\n\r\n") {
                let length = text[..head_end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if request.len() >= head_end + 4 + length || n == 0 {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let reply =
            format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        stream.write_all(reply.as_bytes()).expect("write");
        String::from_utf8_lossy(&request).into_owned()
    });
    (url, handle)
}

#[tokio::test]
async fn the_tracker_posts_the_payload_with_the_client_id_and_no_secret() {
    let (url, server) = serve_once(202);
    let config = OpenPanelConfig::new(&url, "client-1");
    let tracker = Tracker::new(&config).expect("tracker");
    tracker
        .deliver(&Payload::track("signed_in", Some("user-42"), []))
        .await
        .expect("delivered");

    let request = server.join().expect("server");
    let lower = request.to_ascii_lowercase();
    assert!(request.starts_with("POST /api/track HTTP/1.1"), "{request}");
    assert!(lower.contains("openpanel-client-id: client-1"), "{request}");
    assert!(!lower.contains("secret"), "{request}");
    assert!(
        lower.contains("content-type: application/json"),
        "{request}"
    );
    let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
    let body: serde_json::Value = serde_json::from_str(body).expect("json body");
    assert_eq!(body["type"], "track");
    assert_eq!(body["payload"]["name"], "signed_in");
    assert_eq!(body["payload"]["profileId"], "user-42");
}

#[tokio::test]
async fn a_rejected_event_is_an_error_but_its_status_is_reported() {
    let (url, server) = serve_once(401);
    let config = OpenPanelConfig::new(&url, DEFAULT_CLIENT_ID);
    let tracker = Tracker::new(&config).expect("tracker");
    assert!(matches!(
        tracker.deliver(&Payload::identify("user-42")).await,
        Err(AnalyticsError::Rejected(401))
    ));
    server.join().expect("server");

    let (url, server) = serve_once(401);
    let config = OpenPanelConfig::new(&url, DEFAULT_CLIENT_ID);
    let tracker = Tracker::new(&config).expect("tracker");
    assert_eq!(
        tracker
            .send(&Payload::track("analytics_test", None, []))
            .await
            .expect("a response"),
        401
    );
    server.join().expect("server");
}
