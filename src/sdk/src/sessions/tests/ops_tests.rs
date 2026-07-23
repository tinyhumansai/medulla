//! Operator-op and data-model tests: parsing an "open session" line into a
//! [`SessionOp`], applying each op through the manager for its status line, and
//! the trivial `impl`s on the session model types (class/policy/driver/phase,
//! keys, records, and turn origins).

use std::sync::Arc;

use crate::daemon::providers::{RunTaskFn, RunTaskResult};
use crate::tinyplace::HarnessProvider;

use super::super::manager::{OpenSession, SessionConfig, SessionManager};
use super::super::ops::SessionOp;
use super::super::types::{
    SessionClass, SessionDriver, SessionKey, SessionPhase, SessionPolicy, SessionRecord, TurnOrigin,
};

// ---------------------------------------------------------------- fixtures ---

/// An executor that always answers `reply`, capturing nothing. Codex is used as
/// the default provider because its unbound turns route onto the one-shot
/// transport, so `apply` can run a real turn without spawning a CLI.
fn ok_executor(reply: &str) -> RunTaskFn {
    let reply = reply.to_string();
    Arc::new(move |options| {
        let reply = reply.clone();
        let provider = options.provider;
        Box::pin(async move {
            Ok(RunTaskResult {
                provider,
                reply,
                events: 1,
                usage: None,
                session_id: None,
            })
        })
    })
}

fn manager(run: RunTaskFn) -> SessionManager {
    SessionManager::new(
        SessionConfig {
            default_provider: HarnessProvider::Codex,
            ..SessionConfig::default()
        },
        run,
    )
}

// -------------------------------------------------------- SessionOp::parse ---

#[test]
fn parse_open_rejects_blank_input_so_the_caller_can_warn() {
    // Blank input must not become a no-op Open; the caller surfaces "empty".
    assert!(SessionOp::parse_open("   \t\n", SessionClass::Unbound).is_none());
    assert!(SessionOp::parse_open("", SessionClass::Bounded).is_none());
}

#[test]
fn parse_open_takes_the_first_token_as_the_conversation() {
    let op = SessionOp::parse_open("  alice  ", SessionClass::Unbound).expect("parses");
    assert_eq!(
        op,
        SessionOp::Open {
            conversation: "alice".to_string(),
            class: SessionClass::Unbound,
            provider: None,
        }
    );
}

#[test]
fn parse_open_reads_a_known_second_token_as_the_provider() {
    let SessionOp::Open {
        conversation,
        provider,
        ..
    } = SessionOp::parse_open("bob codex", SessionClass::Bounded).expect("parses")
    else {
        panic!("expected an Open");
    };
    assert_eq!(conversation, "bob");
    assert_eq!(provider, Some(HarnessProvider::Codex));
}

#[test]
fn parse_open_ignores_an_unknown_provider_token() {
    // An unrecognized second token is not a provider; the harness selection
    // falls back to the manager's default rather than erroring.
    let SessionOp::Open { provider, .. } =
        SessionOp::parse_open("carol wingdings", SessionClass::Unbound).expect("parses")
    else {
        panic!("expected an Open");
    };
    assert_eq!(provider, None);
}

// ------------------------------------------------------ SessionManager::apply ---

#[tokio::test]
async fn applying_open_registers_the_session_and_describes_its_lifetime() {
    let mgr = manager(ok_executor("ok"));

    let unbound = mgr
        .apply(SessionOp::Open {
            conversation: "alice".to_string(),
            class: SessionClass::Unbound,
            provider: None,
        })
        .await
        .expect("open succeeds");
    assert!(
        unbound.contains("converse across turns"),
        "unbound status: {unbound}"
    );
    assert_eq!(mgr.records().len(), 1, "the session must now exist");

    let bounded = mgr
        .apply(SessionOp::Open {
            conversation: "one-shot".to_string(),
            class: SessionClass::Bounded,
            provider: None,
        })
        .await
        .expect("open succeeds");
    assert!(
        bounded.contains("one turn, then gone"),
        "bounded status: {bounded}"
    );
}

#[tokio::test]
async fn applying_submit_runs_the_turn_and_reports_completion() {
    let mgr = manager(ok_executor("done"));
    let id = mgr.open(OpenSession::operator("alice"));

    let status = mgr
        .apply(SessionOp::Submit {
            id: id.clone(),
            text: "do it".to_string(),
        })
        .await
        .expect("the turn runs");
    assert_eq!(status, format!("{id} · turn complete"));
    assert_eq!(mgr.record(&id).unwrap().turns, 1);
}

#[tokio::test]
async fn applying_submit_to_a_closed_session_is_an_error() {
    // apply forwards the manager's rejection rather than inventing a status.
    let mgr = manager(ok_executor("ok"));
    let id = mgr.open(OpenSession::operator("alice"));
    mgr.close(&id).await;

    let result = mgr
        .apply(SessionOp::Submit {
            id: id.clone(),
            text: "hello".to_string(),
        })
        .await;
    assert!(result.is_err(), "a closed session takes no turn");
}

#[tokio::test]
async fn applying_interrupt_with_no_turn_in_flight_says_so() {
    let mgr = manager(ok_executor("ok"));
    let id = mgr.open(OpenSession::operator("alice"));

    let status = mgr
        .apply(SessionOp::Interrupt { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(status, format!("{id} · no turn in flight to interrupt"));
}

#[tokio::test]
async fn applying_reset_reports_whether_there_was_context_to_drop() {
    let mgr = manager(ok_executor("ok"));
    let id = mgr.open(OpenSession::operator("alice"));

    // Nothing bound yet.
    let cold = mgr
        .apply(SessionOp::Reset { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(cold, format!("{id} · no bound context to reset"));

    // A completed turn binds the codex thread; now a reset has something to do.
    mgr.submit(&id, "first").await.unwrap();
    mgr.registry().record(
        &SessionKey::new("alice", HarnessProvider::Codex),
        "thread-x",
    );
    let warm = mgr
        .apply(SessionOp::Reset { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(
        warm,
        format!("{id} · context dropped; the next turn starts fresh")
    );
}

#[tokio::test]
async fn applying_close_marks_the_session_closed() {
    let mgr = manager(ok_executor("ok"));
    let id = mgr.open(OpenSession::operator("alice"));

    let status = mgr
        .apply(SessionOp::Close { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(status, format!("{id} · closed"));
    assert_eq!(mgr.record(&id).unwrap().phase, SessionPhase::Closed);
}

#[tokio::test]
async fn applying_forget_refuses_a_live_session_then_accepts_a_closed_one() {
    let mgr = manager(ok_executor("ok"));
    let id = mgr.open(OpenSession::operator("alice"));

    let refused = mgr
        .apply(SessionOp::Forget { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(refused, format!("{id} · close it before forgetting it"));
    assert!(mgr.record(&id).is_some(), "still on screen");

    mgr.close(&id).await;
    let forgotten = mgr
        .apply(SessionOp::Forget { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(forgotten, format!("{id} · forgotten"));
    assert!(mgr.record(&id).is_none(), "dropped from the list");
}

// The errored/interrupted `Submit` arms and the `Interrupt` success arm only
// arise on the live interactive transport, so drive `apply` over a fake
// `/bin/sh` claude (the sibling manager tests' harness). Unix-only.
#[cfg(unix)]
use super::manager_tests::claude_harness_manager;

#[cfg(unix)]
#[tokio::test]
async fn applying_submit_reports_a_harness_error() {
    let body = r#"while IFS= read -r l; do case "$l" in *control_request*) : ;; *) printf '{"type":"result","result":"boom","is_error":true}\n' ;; esac; done"#;
    let (mgr, _dir) = claude_harness_manager(body);
    let id = mgr.open(OpenSession::operator("alice"));
    let op = SessionOp::Submit {
        id: id.clone(),
        text: "go".to_string(),
    };
    let status = tokio::time::timeout(std::time::Duration::from_secs(30), mgr.apply(op))
        .await
        .expect("settle")
        .expect("a settled error turn");
    assert_eq!(status, format!("{id} · turn reported an error"));
}
#[cfg(unix)]
#[tokio::test]
async fn applying_interrupt_to_a_running_turn_reports_success_and_aborts_it() {
    let body = r#"while IFS= read -r l; do case "$l" in *control_request*) printf '{"type":"result","subtype":"error_during_execution","is_error":true}\n' ;; *) printf '{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}\n' ;; esac; done"#;
    let (mgr, _dir) = claude_harness_manager(body);
    let id = mgr.open(OpenSession::operator("alice"));
    let background = {
        let (mgr, id) = (mgr.clone(), id.clone());
        let op = SessionOp::Submit {
            id,
            text: "work".to_string(),
        };
        tokio::spawn(async move { mgr.apply(op).await })
    };
    // Wait until the turn is in flight, then let it stream a fragment.
    for _ in 0..2_000 {
        if mgr.record(&id).map(|r| r.phase) == Some(SessionPhase::Turn) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let sent = mgr
        .apply(SessionOp::Interrupt { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(
        sent,
        format!("{id} · interrupt sent; the session stays live")
    );
    let status = tokio::time::timeout(std::time::Duration::from_secs(30), background)
        .await
        .expect("settles promptly")
        .expect("no panic")
        .expect("a settled interrupted turn");
    assert_eq!(status, format!("{id} · turn interrupted"));
}

// --------------------------------------------------------------- SessionClass ---

#[test]
fn only_unbound_sessions_serialize_and_persist() {
    // Both properties are load-bearing: serialization prevents interleaved
    // transcripts, persistence is what makes a session attachable.
    assert!(SessionClass::Unbound.serializes());
    assert!(SessionClass::Unbound.is_persistent());
    assert!(!SessionClass::Bounded.serializes());
    assert!(!SessionClass::Bounded.is_persistent());
}

#[test]
fn session_class_renders_its_wire_string() {
    assert_eq!(SessionClass::Bounded.as_str(), "bounded");
    assert_eq!(SessionClass::Unbound.as_str(), "unbound");
    assert_eq!(SessionClass::Unbound.to_string(), "unbound");
}

// -------------------------------------------------------------- SessionPolicy ---

#[test]
fn session_policy_parse_accepts_every_alias_and_defaults_on_the_unknown() {
    // The aliases are the whole point: a peer may say "pool"/"task" or
    // "conversation"/"interactive" and still route correctly, and anything
    // unrecognized falls back to Auto rather than wedging.
    for pinned in ["bounded", "pool", "task", "  BOUNDED  "] {
        assert_eq!(
            SessionPolicy::parse(pinned),
            SessionPolicy::Bounded,
            "{pinned}"
        );
    }
    for pinned in ["unbound", "conversation", "interactive", "UNBOUND"] {
        assert_eq!(
            SessionPolicy::parse(pinned),
            SessionPolicy::Unbound,
            "{pinned}"
        );
    }
    for other in ["auto", "", "nonsense"] {
        assert_eq!(SessionPolicy::parse(other), SessionPolicy::Auto, "{other}");
    }
    assert_eq!(SessionPolicy::default(), SessionPolicy::Auto);
}

#[test]
fn session_policy_renders_its_wire_string() {
    assert_eq!(SessionPolicy::Auto.as_str(), "auto");
    assert_eq!(SessionPolicy::Bounded.as_str(), "bounded");
    assert_eq!(SessionPolicy::Unbound.as_str(), "unbound");
}

// -------------------------------------------------------------- SessionDriver ---

#[test]
fn session_driver_renders_its_wire_string() {
    assert_eq!(SessionDriver::Task.as_str(), "task");
    assert_eq!(SessionDriver::Envelope.as_str(), "envelope");
    assert_eq!(SessionDriver::Task.to_string(), "task");
}

// ---------------------------------------------------------------- SessionKey ---

#[test]
fn a_session_keys_map_key_is_provider_first_for_exact_lookups() {
    // Provider-first with a space separator means a lookup is an exact string
    // match and never a suffix scan — resetting "bob" must not touch a peer
    // whose id ends in "bob".
    let key = SessionKey::new("bob", HarnessProvider::Codex);
    assert_eq!(key.map_key(), "codex bob");
    assert_ne!(
        key.map_key(),
        SessionKey::new("bob", HarnessProvider::Claude).map_key(),
        "the same peer on two providers holds two independent sessions"
    );
}

#[test]
fn a_session_key_displays_provider_and_conversation() {
    let key = SessionKey::new("alice", HarnessProvider::Claude);
    assert_eq!(key.to_string(), "claude·alice");
}

// ---------------------------------------------------------------- SessionPhase ---

#[test]
fn only_closed_and_failed_phases_are_terminal() {
    for phase in [SessionPhase::Closed, SessionPhase::Failed] {
        assert!(phase.is_terminal(), "{phase:?} must be terminal");
    }
    for phase in [
        SessionPhase::Idle,
        SessionPhase::Starting,
        SessionPhase::Live,
        SessionPhase::Turn,
        SessionPhase::Interrupting,
    ] {
        assert!(!phase.is_terminal(), "{phase:?} is not terminal");
    }
}

#[test]
fn only_idle_and_live_phases_accept_a_new_turn() {
    // Submitting while starting/mid-turn/interrupting would race the running
    // turn, so only a settled session accepts one.
    assert!(SessionPhase::Idle.accepts_turn());
    assert!(SessionPhase::Live.accepts_turn());
    for phase in [
        SessionPhase::Starting,
        SessionPhase::Turn,
        SessionPhase::Interrupting,
        SessionPhase::Closed,
        SessionPhase::Failed,
    ] {
        assert!(!phase.accepts_turn(), "{phase:?} must reject a turn");
    }
}

#[test]
fn every_phase_has_a_distinct_glyph_and_wire_string() {
    let phases = [
        SessionPhase::Idle,
        SessionPhase::Starting,
        SessionPhase::Live,
        SessionPhase::Turn,
        SessionPhase::Interrupting,
        SessionPhase::Closed,
        SessionPhase::Failed,
    ];
    let mut glyphs: Vec<char> = phases.iter().map(|p| p.glyph()).collect();
    glyphs.sort_unstable();
    glyphs.dedup();
    assert_eq!(glyphs.len(), phases.len(), "glyphs must be unambiguous");
    // Every phase renders its stable wire string.
    assert_eq!(SessionPhase::Idle.as_str(), "idle");
    assert_eq!(SessionPhase::Starting.as_str(), "starting");
    assert_eq!(SessionPhase::Live.as_str(), "live");
    assert_eq!(SessionPhase::Turn.as_str(), "turn");
    assert_eq!(SessionPhase::Closed.as_str(), "closed");
    assert_eq!(SessionPhase::Failed.as_str(), "failed");
    assert_eq!(SessionPhase::Interrupting.to_string(), "interrupting");
}

// --------------------------------------------------------------- SessionRecord ---

fn record(class: SessionClass, phase: SessionPhase, last_at: i64) -> SessionRecord {
    SessionRecord {
        id: "s_1".to_string(),
        key: SessionKey::new("alice", HarnessProvider::Claude),
        class,
        driver: SessionDriver::Task,
        phase,
        workspace: "/repo".to_string(),
        harness_session_id: None,
        turns: 0,
        created_at: 0,
        last_at,
        last_error: None,
    }
}

#[test]
fn idle_ms_is_the_clamped_gap_since_the_last_activity() {
    let rec = record(SessionClass::Unbound, SessionPhase::Live, 1_000);
    assert_eq!(rec.idle_ms(1_500), 500);
    // A clock that reads before the last activity must never go negative.
    assert_eq!(rec.idle_ms(400), 0);
}

#[test]
fn only_a_live_unbound_session_is_attachable() {
    // Attachable = unbound and not terminal. A bounded session is gone on its
    // reply; a closed unbound one has nothing to converse with.
    assert!(record(SessionClass::Unbound, SessionPhase::Live, 0).is_attachable());
    assert!(!record(SessionClass::Bounded, SessionPhase::Live, 0).is_attachable());
    assert!(!record(SessionClass::Unbound, SessionPhase::Closed, 0).is_attachable());
}

// ----------------------------------------------------------------- TurnOrigin ---

#[test]
fn a_turn_origins_kind_and_driver_track_its_variant() {
    let frame = TurnOrigin::Frame {
        task_id: "t1".to_string(),
        correlation_id: None,
    };
    assert_eq!(frame.as_str(), "frame");
    assert_eq!(frame.driver(), SessionDriver::Task);

    let envelope = TurnOrigin::Envelope {
        event_id: "e".to_string(),
        seq: 3,
    };
    assert_eq!(envelope.as_str(), "envelope");
    assert_eq!(envelope.driver(), SessionDriver::Envelope);

    // An operator turn is typed into a live session — the same path a frame
    // drives — so it reports the Task driver despite its own kind string.
    assert_eq!(TurnOrigin::Operator.as_str(), "operator");
    assert_eq!(TurnOrigin::Operator.driver(), SessionDriver::Task);
}
