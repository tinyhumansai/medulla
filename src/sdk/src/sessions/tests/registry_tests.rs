//! Binding-registry tests: the plan/record/reset lifecycle, per-provider
//! isolation, LRU eviction, and which turns take the conversation chain.

use crate::tinyplace::HarnessProvider;

use super::super::registry::SessionRegistry;
use super::super::types::{SessionClass, SessionKey};

// --------------------------------------------------------------- registry ---

/// A key for `conversation` on `provider`.
fn key(conversation: &str, provider: HarnessProvider) -> SessionKey {
    SessionKey::new(conversation, provider)
}

#[test]
fn a_bounded_turn_never_touches_the_binding_map() {
    let registry = SessionRegistry::default();
    let alice = key("alice", HarnessProvider::Claude);
    registry.record(&alice, "sess-1");

    let plan = registry.plan(&alice, SessionClass::Bounded);
    assert_eq!(plan.resume_session_id, None, "bounded never resumes");
    assert!(!plan.bind, "bounded never binds");
}

#[test]
fn the_first_unbound_turn_binds_and_the_second_resumes() {
    let registry = SessionRegistry::default();
    let alice = key("alice", HarnessProvider::Claude);

    let first = registry.plan(&alice, SessionClass::Unbound);
    assert!(first.bind, "the first turn is the bounded→unbound edge");
    assert_eq!(first.resume_session_id, None);

    registry.record(&alice, "sess-1");
    let second = registry.plan(&alice, SessionClass::Unbound);
    assert!(!second.bind);
    assert_eq!(second.resume_session_id.as_deref(), Some("sess-1"));
}

#[test]
fn bindings_are_per_provider() {
    // The same peer on claude and codex holds two independent sessions: a
    // session id from one CLI is meaningless to the other.
    let registry = SessionRegistry::default();
    registry.record(&key("alice", HarnessProvider::Claude), "claude-1");

    let codex = registry.plan(&key("alice", HarnessProvider::Codex), SessionClass::Unbound);
    assert!(codex.bind, "codex must not inherit claude's session");
    assert_eq!(codex.resume_session_id, None);
}

#[test]
fn a_provider_that_cannot_resume_never_binds() {
    let registry = SessionRegistry::default();
    let plan = registry.plan(
        &key("alice", HarnessProvider::Opencode),
        SessionClass::Unbound,
    );
    assert!(!plan.bind, "recording an unusable binding would mislead");
    assert!(registry.is_empty());
}

#[test]
fn reset_drops_only_the_exact_conversation() {
    // The bug this guards: a suffix scan would let resetting `bob` wipe `alicebob`.
    let registry = SessionRegistry::default();
    registry.record(&key("bob", HarnessProvider::Claude), "s1");
    registry.record(&key("alicebob", HarnessProvider::Claude), "s2");

    assert!(registry.reset(&key("bob", HarnessProvider::Claude)));
    assert_eq!(
        registry.bound(&key("alicebob", HarnessProvider::Claude)),
        Some("s2".to_string())
    );
}

#[test]
fn resetting_a_conversation_clears_every_provider() {
    let registry = SessionRegistry::default();
    registry.record(&key("alice", HarnessProvider::Claude), "s1");
    registry.record(&key("alice", HarnessProvider::Codex), "s2");
    registry.record(&key("alicebob", HarnessProvider::Claude), "s3");

    assert_eq!(registry.reset_conversation("alice"), 2);
    assert!(registry
        .bound(&key("alicebob", HarnessProvider::Claude))
        .is_some());
}

#[test]
fn bindings_evict_least_recently_used_first() {
    let registry = SessionRegistry::new(2);
    registry.record(&key("a", HarnessProvider::Claude), "s1");
    registry.record(&key("b", HarnessProvider::Claude), "s2");
    // Re-recording `a` refreshes its recency, so `b` becomes the oldest.
    registry.record(&key("a", HarnessProvider::Claude), "s1b");
    registry.record(&key("c", HarnessProvider::Claude), "s3");

    assert_eq!(registry.len(), 2);
    assert!(registry.bound(&key("b", HarnessProvider::Claude)).is_none());
    assert_eq!(
        registry.bound(&key("a", HarnessProvider::Claude)),
        Some("s1b".to_string())
    );
}

#[test]
fn an_empty_session_id_is_never_recorded_as_a_binding() {
    // A harness that announced no id must not leave a blank binding that a later
    // turn would try (and fail) to resume.
    let registry = SessionRegistry::default();
    let alice = key("alice", HarnessProvider::Claude);
    registry.record(&alice, "   ");
    assert!(registry.is_empty(), "a blank id is not a binding");
    assert_eq!(registry.bound(&alice), None);

    // A real id still records normally.
    registry.record(&alice, "sess-1");
    assert_eq!(registry.bound(&alice).as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn a_released_chain_is_pruned_and_can_be_reacquired() {
    // prune_chain drops a chain only when no turn still holds it, so a closed
    // conversation stops leaking its per-key mutex — but a later turn can still
    // open a fresh one.
    let registry = SessionRegistry::default();
    let alice = key("alice", HarnessProvider::Claude);

    let held = registry
        .acquire_turn(&alice, SessionClass::Unbound)
        .await
        .expect("unbound turns serialize");
    // While the guard is live the chain must survive a prune attempt.
    registry.prune_chain(&alice);
    drop(held);

    // Released, it is now pruned; acquiring again must still succeed.
    registry.prune_chain(&alice);
    assert!(
        registry
            .acquire_turn(&alice, SessionClass::Unbound)
            .await
            .is_some(),
        "a pruned chain is transparently recreated on the next turn"
    );
}

#[tokio::test]
async fn only_unbound_turns_take_the_conversation_chain() {
    let registry = SessionRegistry::default();
    let alice = key("alice", HarnessProvider::Claude);

    let held = registry.acquire_turn(&alice, SessionClass::Unbound).await;
    assert!(held.is_some(), "unbound turns serialize");

    // A bounded turn must not queue behind it — that would turn the daemon's
    // concurrency budget into a single file.
    let bounded = registry.acquire_turn(&alice, SessionClass::Bounded).await;
    assert!(bounded.is_none());
}
