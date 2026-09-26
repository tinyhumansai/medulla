//! Unit tests for the remote-control envelope.
//!
//! The property that matters most is mutual exclusion with the screen protocol:
//! the two share a channel, and each parser must decline the other's bodies
//! rather than half-decoding them.

use super::*;
use crate::protocol::{encode_screen_message, parse_screen_message, ScreenMessage};

fn capabilities() -> RemoteCapabilities {
    RemoteCapabilities {
        version: "0.12.0".to_string(),
        harnesses: vec![RemoteHarnessChoice {
            id: "shell:zsh".to_string(),
            provider: "shell".to_string(),
            preset: None,
            display_name: "zsh".to_string(),
        }],
        workspace: "/home/steven/src".to_string(),
        workspaces: vec!["/home/steven/other".to_string()],
        host_name: "tower".to_string(),
    }
}

fn row() -> RemoteSessionRow {
    RemoteSessionRow {
        id: "w_1".to_string(),
        label: "you:shell:zsh".to_string(),
        provider: "shell".to_string(),
        preset: None,
        state: RemoteSessionState::Running,
        cwd: "/home/steven/src".to_string(),
        name: Some("build".to_string()),
        thread_name: None,
        started_at: 1_700_000_000_000,
        last_output_at: 1_700_000_001_000,
        busy: false,
        working: false,
        attention: None,
        bracketed_paste: true,
        last_error: None,
    }
}

#[test]
fn every_message_round_trips() {
    let messages = [
        RemoteMessage::Hello {
            client_version: "0.12.0".to_string(),
        },
        RemoteMessage::Capabilities(capabilities()),
        RemoteMessage::Open {
            request_id: "r1".to_string(),
            harness: capabilities().harnesses[0].clone(),
            workspace: "/tmp".to_string(),
            cols: 120,
            rows: 30,
            name: Some("scratch".to_string()),
        },
        RemoteMessage::Opened {
            request_id: "r1".to_string(),
            session_id: "w_1".to_string(),
        },
        RemoteMessage::OpenFailed {
            request_id: "r1".to_string(),
            reason: "/nope is not a directory".to_string(),
        },
        RemoteMessage::Close {
            session_id: "w_1".to_string(),
        },
        RemoteMessage::Sessions { rows: vec![row()] },
    ];
    for message in messages {
        let parsed = parse_remote_message(&encode_remote_message(&message))
            .expect("a remote message is one of ours");
        assert_eq!(parsed, message);
    }
}

#[test]
fn an_exited_session_carries_its_code() {
    let mut exited = row();
    exited.state = RemoteSessionState::Exited { code: Some(130) };
    let message = RemoteMessage::Sessions { rows: vec![exited] };
    let parsed = parse_remote_message(&encode_remote_message(&message)).unwrap();
    let RemoteMessage::Sessions { rows } = parsed else {
        panic!("expected a session list");
    };
    assert_eq!(
        rows[0].state,
        RemoteSessionState::Exited { code: Some(130) }
    );
}

#[test]
fn a_screen_message_is_not_parsed_as_a_remote_one() {
    // Both ride channel 0 and arrive at the same reader, so each parser has to
    // decline the other's bodies rather than half-decoding them.
    let body = encode_screen_message(&ScreenMessage::input("w_1", b"ls\r"));
    assert!(parse_remote_message(&body).is_none());
}

#[test]
fn a_remote_message_is_not_parsed_as_a_screen_one() {
    let body = encode_remote_message(&RemoteMessage::Close {
        session_id: "w_1".to_string(),
    });
    assert!(parse_screen_message(&body).is_none());
}

#[test]
fn junk_is_declined_rather_than_panicking() {
    // Inbound bodies are untrusted: anything unparseable must fall through to
    // the next protocol, not take the reader down with it.
    for body in ["", "not json", "{}", r#"{"remote_version":"other"}"#, "[]"] {
        assert!(parse_remote_message(body).is_none(), "accepted {body:?}");
    }
}

#[test]
fn an_unknown_kind_is_declined_rather_than_guessed() {
    let body = r#"{"remote_version":"medulla.remote.v1","kind":"teleport"}"#;
    assert!(parse_remote_message(body).is_none());
}
