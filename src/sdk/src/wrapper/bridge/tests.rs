//! Unit tests for the bridge's inbound classification.
//!
//! The predicate under test decides what may be typed into a live harness, so
//! its failure mode is not a wrong value but a structured frame executed as a
//! prompt. Each protocol sharing the channel gets a case.

use std::collections::HashMap;

use super::{is_structured_frame, wrapper_line_mapper};
use crate::protocol::{
    encode_harness_control_frame, encode_screen_message, ScreenMessage, SCREEN_PROTO,
};

/// Every screen message kind is structured, whichever direction it travels.
///
/// The regression: `medulla.screen.v1` was the one protocol on this channel the
/// predicate did not recognise, so an owner's `subscribe` was injected into the
/// child as prompt text instead of being handled by the screen router.
#[test]
fn screen_messages_are_structured() {
    let cases = vec![
        ScreenMessage::Subscribe {
            task_id: "locate-oh-repo:t1#0".into(),
            max_fps: 1,
            resync: true,
        },
        ScreenMessage::Unsubscribe {
            task_id: "locate-oh-repo:t1#0".into(),
        },
        ScreenMessage::Ack {
            task_id: "locate-oh-repo:t1#0".into(),
            seq: 7,
        },
    ];
    for message in cases {
        let body = encode_screen_message(&message);
        assert!(
            is_structured_frame(&body),
            "a screen message must never be injected: {body}"
        );
    }
}

/// The exact body observed reaching a harness prompt, pinned as written.
///
/// Encoding a `Subscribe` would pass even if field order or spelling drifted;
/// this is the literal wire text that was mis-injected.
#[test]
fn the_observed_subscribe_body_is_structured() {
    let body = format!(
        r#"{{"screen_version":"{SCREEN_PROTO}","kind":"subscribe","task_id":"locate-oh-repo:t1#0","max_fps":1,"resync":true}}"#
    );
    assert!(is_structured_frame(&body));
}

/// A control frame is *not* structured by this predicate, and that is correct:
/// it is claimed earlier, and its `text` is exactly what the owner means to
/// inject. Pinned so the two paths are not accidentally merged.
#[test]
fn control_frames_are_left_to_the_earlier_branch() {
    let body = encode_harness_control_frame("run the tests", Some("s_1"));
    assert!(!is_structured_frame(&body));
}

/// Ordinary owner DMs are what the injection path exists to carry.
#[test]
fn plain_text_is_not_structured() {
    assert!(!is_structured_frame("please locate the openhuman repo"));
    assert!(!is_structured_frame("  not { json"));
    assert!(!is_structured_frame(""));
    // Prose that merely mentions the protocol is still prose.
    assert!(!is_structured_frame(
        "the medulla.screen.v1 subscribe never arrived"
    ));
}

/// A wrapper environment override is part of the child's effective environment,
/// even when it is absent from the process running the SDK.
#[test]
fn configured_github_repository_override_reaches_the_mapper() {
    let env = HashMap::from([("GH_REPO".to_string(), "other/project".to_string())]);
    let mut mapper = wrapper_line_mapper("claude", &env, false);
    let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"pr-1","name":"Bash","input":{"command":"gh pr create --fill"}}]}}"#;
    let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"pr-1","content":"https://github.com/other/project/pull/7"}]}}"#;
    mapper.map_line(call, 0);
    assert!(mapper.map_line(result, 1).iter().all(|event| event
        .event
        .payload
        .get("pull_request")
        .is_none()));
}
