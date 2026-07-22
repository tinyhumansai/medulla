//! Routing tests: which lifetime class a stimulus gets, and which transport
//! follows from that class and the provider's capabilities.

use crate::tinyplace::HarnessProvider;

use super::super::routing::{
    can_resume, can_run_interactive, has_continuity, route_session_class, route_transport,
    Stimulus, Transport,
};
use super::super::types::{SessionClass, SessionPolicy};

// ---------------------------------------------------------------- routing ---

#[test]
fn task_frames_route_bounded_and_conversation_routes_unbound() {
    // The heart of `auto`: discrete work must not inherit conversational
    // context, and a conversation must not forget between messages.
    assert_eq!(
        route_session_class(Stimulus::Task, None, SessionPolicy::Auto),
        SessionClass::Bounded
    );
    assert_eq!(
        route_session_class(Stimulus::PlainText, None, SessionPolicy::Auto),
        SessionClass::Unbound
    );
    assert_eq!(
        route_session_class(Stimulus::Operator, None, SessionPolicy::Auto),
        SessionClass::Unbound
    );
}

#[test]
fn a_requested_class_outranks_the_operator_policy() {
    // The sender knows what it wants; a pin is only a default for frames that
    // ask for nothing.
    assert_eq!(
        route_session_class(
            Stimulus::Task,
            Some(SessionClass::Unbound),
            SessionPolicy::Bounded
        ),
        SessionClass::Unbound
    );
    assert_eq!(
        route_session_class(Stimulus::PlainText, None, SessionPolicy::Bounded),
        SessionClass::Bounded
    );
}

#[test]
fn an_unbound_policy_pin_overrides_a_task_stimulus() {
    // A task frame is discrete work and routes bounded by default; an explicit
    // Unbound pin must win, so an operator can force conversations for tasks.
    assert_eq!(
        route_session_class(Stimulus::Task, None, SessionPolicy::Unbound),
        SessionClass::Unbound
    );
    assert_eq!(
        route_session_class(Stimulus::Operator, None, SessionPolicy::Unbound),
        SessionClass::Unbound
    );
}

#[test]
fn transport_renders_its_wire_string() {
    assert_eq!(Transport::OneShot.as_str(), "one-shot");
    assert_eq!(Transport::Interactive.as_str(), "interactive");
}

#[test]
fn unknown_policy_names_fall_back_to_auto() {
    assert_eq!(SessionPolicy::parse("nonsense"), SessionPolicy::Auto);
    assert_eq!(SessionPolicy::parse("conversation"), SessionPolicy::Unbound);
    assert_eq!(SessionPolicy::parse("pool"), SessionPolicy::Bounded);
}

#[test]
fn only_unbound_claude_gets_the_interactive_transport() {
    // Bounded is always one-shot: a single turn gains nothing from a persistent
    // process.
    assert_eq!(
        route_transport(SessionClass::Bounded, HarnessProvider::Claude),
        Transport::OneShot
    );
    assert_eq!(
        route_transport(SessionClass::Unbound, HarnessProvider::Claude),
        Transport::Interactive
    );
    // codex/opencode degrade rather than fail — they are still real sessions.
    assert_eq!(
        route_transport(SessionClass::Unbound, HarnessProvider::Codex),
        Transport::OneShot
    );
    assert_eq!(
        route_transport(SessionClass::Unbound, HarnessProvider::Opencode),
        Transport::OneShot
    );
}

#[test]
fn opencode_is_the_only_provider_with_no_continuity_at_all() {
    assert!(can_run_interactive(HarnessProvider::Claude));
    assert!(!can_run_interactive(HarnessProvider::Codex));
    assert!(can_resume(HarnessProvider::Codex));
    assert!(!can_resume(HarnessProvider::Opencode));
    assert!(has_continuity(HarnessProvider::Claude));
    assert!(has_continuity(HarnessProvider::Codex));
    assert!(!has_continuity(HarnessProvider::Opencode));
}
