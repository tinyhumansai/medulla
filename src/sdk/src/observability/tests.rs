//! Unit tests for crash-report configuration, scrubbing, and transport.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use sentry::protocol::{Breadcrumb, Event, Exception, Frame, LogEntry, Stacktrace, User};

use super::config::{is_opted_out, release, resolve_dsn, resolve_environment, DEFAULT_DSN};
use super::scrub::{scrub_event, scrub_paths, scrub_text};
use super::{current_user, set_user, transport, CrashReportingStatus};

#[test]
fn the_runtime_dsn_overrides_the_compiled_in_default() {
    assert_eq!(
        resolve_dsn(Some(" https://k@rt.example/1 ")),
        "https://k@rt.example/1"
    );
}

#[test]
fn every_build_carries_the_medulla_project_dsn() {
    assert_eq!(resolve_dsn(None), DEFAULT_DSN);
    assert_eq!(
        DEFAULT_DSN,
        "https://40b6883c6f8013c8382f8b0bc5986108@sentry.tinyhumans.ai/12"
    );
    assert!(DEFAULT_DSN.parse::<sentry::types::Dsn>().is_ok());
    // A blank override falls back to the default rather than breaking it.
    assert_eq!(resolve_dsn(Some("  ")), DEFAULT_DSN);
}

#[test]
fn this_crates_tests_never_start_real_crash_reporting() {
    assert_eq!(
        super::resolve_status(),
        (CrashReportingStatus::Disabled, None)
    );
}

#[test]
fn only_a_truthy_opt_out_disables_reporting() {
    for value in ["1", "true", "TRUE", " yes "] {
        assert!(is_opted_out(Some(value)), "{value:?} should opt out");
    }
    for value in ["", "0", "false", "no", "off"] {
        assert!(!is_opted_out(Some(value)), "{value:?} should not opt out");
    }
    assert!(!is_opted_out(None));
}

#[test]
fn release_is_tagged_with_the_crate_version() {
    assert_eq!(release(), format!("medulla@{}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn environment_defaults_by_build_profile_and_accepts_an_override() {
    assert_eq!(resolve_environment(None, true), "development");
    assert_eq!(resolve_environment(Some(" "), false), "production");
    assert_eq!(resolve_environment(Some("Staging"), false), "staging");
}

#[test]
fn home_directory_paths_are_replaced_with_a_tilde() {
    let scrubbed = scrub_paths(
        "failed to open /home/alice/project/src/main.rs",
        Some("/home/alice"),
    );
    assert_eq!(scrubbed, "failed to open ~/project/src/main.rs");
}

#[test]
fn the_home_prefix_only_matches_a_whole_path_component() {
    // `/home/al` must not swallow part of `/home/alice`; the generic mask
    // still hides the other user's name.
    assert_eq!(
        scrub_paths("/home/alice/x", Some("/home/al")),
        "/home/<user>/x"
    );
}

#[test]
fn other_user_directories_are_masked_on_every_platform() {
    assert_eq!(
        scrub_paths("/Users/bob/.medulla", None),
        "/Users/<user>/.medulla"
    );
    assert_eq!(
        scrub_paths(r"C:\Users\carol\AppData\medulla.exe", None),
        r"C:\Users\<user>\AppData\medulla.exe"
    );
}

#[test]
fn credentials_in_messages_are_redacted() {
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl";
    let scrubbed = scrub_text(
        &format!("auth failed: Bearer abcdefghijklmnop, token {jwt}"),
        None,
    );
    assert!(!scrubbed.contains("abcdefghijklmnop"), "{scrubbed}");
    assert!(!scrubbed.contains(jwt), "{scrubbed}");
    assert!(scrubbed.contains("Bearer <redacted>"), "{scrubbed}");
}

#[test]
fn long_messages_are_truncated_on_a_char_boundary() {
    let scrubbed = scrub_text(&"é".repeat(2000), None);
    assert!(scrubbed.len() <= 1024 + '…'.len_utf8());
    assert!(scrubbed.ends_with('…'));
}

#[test]
fn events_lose_everything_that_identifies_the_person() {
    let frame = Frame {
        abs_path: Some("/home/alice/src/medulla/src/main.rs".into()),
        filename: Some("/home/alice/src/medulla/src/main.rs".into()),
        package: Some("/home/alice/.cargo/bin/medulla".into()),
        context_line: Some("let prompt = \"secret\";".into()),
        pre_context: vec!["line".into()],
        vars: [("prompt".to_owned(), "secret plans".into())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let mut event = Event {
        server_name: Some("alices-laptop".into()),
        message: Some("panicked at /home/alice/src/x.rs".into()),
        logentry: Some(LogEntry {
            message: "read {}".into(),
            params: vec!["/home/alice/notes.txt".into()],
        }),
        user: Some(User {
            id: Some("user-42".into()),
            email: Some("alice@example.com".into()),
            username: Some("alice".into()),
            ..Default::default()
        }),
        extra: [("argv".to_owned(), "--config secret.toml".into())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    event.breadcrumbs.values.push(Breadcrumb::default());
    event.exception.values.push(Exception {
        ty: "panic".into(),
        value: Some("open /home/alice/secret.txt".into()),
        stacktrace: Some(Stacktrace {
            frames: vec![frame],
            ..Default::default()
        }),
        ..Default::default()
    });

    let event = scrub_event(event, Some("/home/alice"));

    assert_eq!(event.server_name, None);
    assert!(event.breadcrumbs.values.is_empty());
    assert!(event.extra.is_empty());
    assert_eq!(event.message.as_deref(), Some("panicked at ~/src/x.rs"));
    let entry = event.logentry.as_ref().expect("logentry kept");
    assert!(entry.params.is_empty());
    let user = event.user.as_ref().expect("account id kept");
    assert_eq!(user.id.as_deref(), Some("user-42"));
    assert_eq!(user.email, None);
    assert_eq!(user.username, None);
    let exception = &event.exception.values[0];
    assert_eq!(exception.value.as_deref(), Some("open ~/secret.txt"));
    let frame = &exception.stacktrace.as_ref().expect("stack").frames[0];
    assert_eq!(frame.abs_path.as_deref(), Some("~/src/medulla/src/main.rs"));
    assert_eq!(frame.package.as_deref(), Some("~/.cargo/bin/medulla"));
    assert!(frame.vars.is_empty());
    assert!(frame.pre_context.is_empty());
    assert_eq!(frame.context_line, None);
}

#[test]
fn the_account_id_can_be_set_and_cleared() {
    set_user(Some("user-42"));
    assert_eq!(current_user().as_deref(), Some("user-42"));
    set_user(None);
    assert_eq!(current_user(), None);
}

#[test]
fn a_test_event_without_a_client_reports_why() {
    // No test calls `init`, so the main hub has no client.
    let error = super::send_test_event(Duration::from_millis(10)).expect_err("no client");
    assert_ne!(error, CrashReportingStatus::Active);
    assert!(!error.to_string().is_empty());
}

#[test]
fn the_transport_posts_envelopes_to_the_dsn_and_records_the_status() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        // Read until the envelope body (which follows the headers) arrives.
        while let Ok(read) = stream.read(&mut buf) {
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if String::from_utf8_lossy(&request).contains("sentry-test-transport") {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .expect("respond");
        String::from_utf8_lossy(&request).into_owned()
    });

    let options = sentry::ClientOptions {
        dsn: Some(
            format!("http://publickey@127.0.0.1:{port}/7")
                .parse()
                .expect("dsn"),
        ),
        ..Default::default()
    };
    let transport = transport::factory(&options);
    let mut envelope = sentry::Envelope::new();
    envelope.add_item(Event {
        message: Some("sentry-test-transport".into()),
        ..Default::default()
    });
    transport.send_envelope(envelope);
    assert!(transport.flush(Duration::from_secs(10)), "flushed");

    let request = server.join().expect("server thread");
    assert!(request.starts_with("POST /api/7/envelope/"), "{request}");
    assert!(request.to_ascii_lowercase().contains("x-sentry-auth"));
    assert_eq!(transport::last_status(), Some(200));
}
