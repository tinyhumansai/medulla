//! What the operator's own door stamps on a session it opens.
//!
//! The other door — a dispatched task — is covered by the executor's session
//! tests. Both are here for the same reason: `origin` is decided once, by the
//! path that created the session, and nothing downstream can recover it if that
//! path stamps the wrong thing.
//!
//! Unix-only, like every test in this module that starts a real child: `/bin/sh`
//! stands in for a coding agent.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

use crate::worker::pty::{PtyManager, SessionControl};

use super::session::harnesses;

/// A [`LocalSessions`](super::super::LocalSessions) whose "codex" is
/// `/bin/sh`, so opening one starts a real pty client and nothing else.
fn shell_harnesses(sessions: PtyManager) -> super::super::LocalSessions {
    let mut harnesses = harnesses(sessions);
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());
    harnesses.env = env;
    harnesses
}

/// The picker's codex entry.
fn codex(harnesses: &super::super::LocalSessions) -> super::super::HarnessChoice {
    harnesses
        .choices()
        .into_iter()
        .find(|choice| choice.provider == HarnessProvider::Codex)
        .expect("codex is a configured provider")
}

#[test]
fn a_session_the_operator_opens_is_user_originated() {
    let sessions = PtyManager::new();
    let harnesses = shell_harnesses(sessions.clone());
    let choice = codex(&harnesses);

    let id = harnesses
        .open_unmanaged(&choice, "/", false)
        .expect("the operator's own harness starts");

    let row = sessions.row(&id).expect("the session exists");
    assert!(row.origin.is_user(), "a person asked for this one");
    assert_eq!(row.control, SessionControl::User, "and holds it");
    assert_eq!(row.name, None, "the picker has no name prompt yet");

    sessions.close(&id);
}

#[test]
fn the_name_a_person_gives_a_session_reaches_its_row() {
    // The seam the picker's name prompt lands on: everything below this call
    // already carries the name through to the row the rail renders.
    let sessions = PtyManager::new();
    let harnesses = shell_harnesses(sessions.clone());
    let choice = codex(&harnesses);

    let id = harnesses
        .open_unmanaged_named(&choice, "/", false, Some("debug login".to_string()))
        .expect("the operator's own harness starts");

    let row = sessions.row(&id).expect("the session exists");
    assert_eq!(row.name.as_deref(), Some("debug login"));
    assert!(row.origin.is_user());

    sessions.close(&id);
}

#[test]
fn opening_in_a_directory_that_does_not_exist_still_fails_the_same_way() {
    // Provenance is threaded through the success path only; the guard that
    // names the folder rather than letting `posix_spawn` report a generic
    // failure is unchanged.
    let sessions = PtyManager::new();
    let harnesses = shell_harnesses(sessions.clone());
    let choice = codex(&harnesses);

    let error = harnesses
        .open_unmanaged_named(&choice, "/no/such/folder", false, Some("x".to_string()))
        .expect_err("a missing directory is refused before the spawn");
    assert!(error.contains("/no/such/folder"), "{error}");
    assert!(sessions.rows().is_empty(), "nothing was started");
}
