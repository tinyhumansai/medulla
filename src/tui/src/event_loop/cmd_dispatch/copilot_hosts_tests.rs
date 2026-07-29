//! Tests for the copilot host cache's bookkeeping.
//!
//! Against the generic helpers rather than the live cache: starting a real
//! [`super::LocalWorkflowHost`] needs a coding CLI on `PATH` and binds loopback
//! endpoints, and none of the behaviour here depends on what a host is. What
//! matters is which key holds which entry and which one the cap drops.

use super::*;

/// Entries as `(thread, marker)`, where the marker only has to be comparable.
fn entries(threads: &[&str]) -> Vec<(String, u32)> {
    threads
        .iter()
        .enumerate()
        .map(|(n, thread)| (thread.to_string(), n as u32))
        .collect()
}

fn keys<T>(entries: &[(String, T)]) -> Vec<&str> {
    entries.iter().map(|(key, _)| key.as_str()).collect()
}

#[test]
fn a_continuing_conversation_gets_the_host_it_already_had() {
    let mut cache = entries(&["sweep", "digest"]);

    let found = touch(&mut cache, "sweep");

    // The same entry, not a new one: a fresh host would be a fresh daemon, and
    // the session the last turn opened lives in the old one.
    assert_eq!(found, Some(0));
}

#[test]
fn touching_makes_a_thread_the_most_recent_so_the_cap_drops_the_idle_one() {
    let mut cache = entries(&["oldest", "middle", "newest"]);

    touch(&mut cache, "oldest");

    assert_eq!(keys(&cache), vec!["middle", "newest", "oldest"]);
}

#[test]
fn a_thread_with_no_host_yet_reports_none_rather_than_guessing() {
    let mut cache = entries(&["sweep"]);

    assert_eq!(touch(&mut cache, "never-opened"), None);
}

#[test]
fn the_cap_evicts_the_least_recently_used_conversation() {
    let mut cache: Vec<(String, u32)> = Vec::new();
    for (n, thread) in ["a", "b", "c", "d"].iter().enumerate() {
        insert(&mut cache, thread, n as u32);
    }

    // Each entry is a live daemon holding harness processes, so an operator
    // walking a large catalogue must not accumulate one per workflow.
    assert_eq!(cache.len(), MAX_LIVE_HOSTS);
    assert_eq!(keys(&cache), vec!["b", "c", "d"]);
}

#[test]
fn renaming_moves_a_conversation_onto_the_workflow_it_created() {
    let mut cache = entries(&["\u{1}new-workflow"]);

    rename_in(&mut cache, "\u{1}new-workflow", "nightly-sweep");

    // Without this the operator's next instruction would be turn one of a
    // session that had never heard of the workflow it just built.
    assert_eq!(keys(&cache), vec!["nightly-sweep"]);
    assert_eq!(cache[0].1, 0, "the same host, under a new key");
}

#[test]
fn renaming_onto_an_occupied_key_replaces_what_was_there() {
    let mut cache = entries(&["stale", "\u{1}new-workflow"]);

    rename_in(&mut cache, "\u{1}new-workflow", "stale");

    // The workflow is new, so anything filed under its id belongs to a deleted
    // one that happened to share it — not a conversation worth continuing.
    assert_eq!(keys(&cache), vec!["stale"]);
    assert_eq!(cache[0].1, 1, "the create turn's host, not the stale one");
}

#[test]
fn renaming_a_thread_that_has_no_host_does_nothing() {
    let mut cache = entries(&["other"]);

    rename_in(&mut cache, "never-started", "fresh");

    assert_eq!(keys(&cache), vec!["other"]);
}

#[test]
fn forgetting_ends_only_the_named_conversation() {
    let mut cache = entries(&["a", "b"]);

    forget_in(&mut cache, "a");

    assert_eq!(keys(&cache), vec!["b"]);
}
