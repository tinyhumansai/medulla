//! Resolving a session to the agent that owns it.
//!
//! The rail's own level, and the only one it resolves for itself: hosts and
//! agents come from the shared projection (see [`super`]). Kept apart from the
//! assembly so the rule can be tested without building an
//! [`App`](super::super::types::App).
//!
//! A **dispatched** session already knows its agent: the hub files a task under
//! the roster id the dispatch named ([`lane_id`]), and that id is the lane's
//! `agent_id`. Nothing is re-derived here — re-deriving it by workspace would
//! disagree with the hub on exactly the case the hub was fixed for (a machine
//! advertising several agents at one address).
//!
//! An **operator-started** session knows only where it is running, so it is
//! matched back to the declaration whose `harness × workspace` it is a session
//! of. No match means the directory is undeclared, which is a real state the
//! rail shows rather than hides — and the prompt for inline agent creation.
//!
//! [`lane_id`]: https://docs.rs/medulla

use medulla::runtime::AgentDeclaration;

use crate::worker::pty::SessionRow;

/// The declaration a local PTY session belongs to, if one claims it.
///
/// Matched on `harness × workspace`, which is what an agent *is*: the CLI that
/// runs the work and the directory it runs in. Paths are compared with trailing
/// separators trimmed, because a declaration typed by hand and a cwd resolved by
/// the spawner disagree about the trailing slash far more often than they
/// disagree about the directory.
///
/// The first match wins. Two declarations of the same harness in the same
/// directory are the same agent declared twice, so which one claims the session
/// changes nothing an operator can see.
pub fn agent_for_session<'a>(
    declarations: &'a [AgentDeclaration],
    row: &SessionRow,
) -> Option<&'a AgentDeclaration> {
    let cwd = normalize_path(&row.cwd);
    let harness = row.provider.as_str();
    declarations.iter().find(|declaration| {
        declaration.harness.trim().eq_ignore_ascii_case(harness)
            && normalize_path(&declaration.workspace.path) == cwd
    })
}

/// A path with its trailing separators removed, for comparison only.
///
/// Never used to *build* a path: an empty result means "the root or nothing",
/// and both compare equal to each other, which is the honest answer for a blank
/// declaration.
fn normalize_path(path: &str) -> &str {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches('/');
    if stripped.is_empty() {
        trimmed
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla::protocol::HarnessProvider;
    use medulla::runtime::WorkspaceRef;

    use crate::worker::pty::{HarnessControl, PtyState, SessionOrigin};

    fn session(provider: HarnessProvider, cwd: &str) -> SessionRow {
        SessionRow {
            id: "w_1".into(),
            label: "local".into(),
            provider,
            state: PtyState::Running,
            cwd: cwd.into(),
            branch: None,
            launch_root: None,
            launch_commit: None,
            launch_checkout_identity: None,
            session_id: None,
            thread_name: None,
            started_at: 1,
            last_output_at: 1,
            last_error: None,
            busy: false,
            control: HarnessControl::User,
            origin: SessionOrigin::User,
            name: None,
            attention: None,
        }
    }

    #[test]
    fn a_session_matches_the_declaration_of_its_harness_and_directory() {
        let declarations = vec![
            AgentDeclaration::new("api-claude", "host", "claude", "/work/api"),
            AgentDeclaration::new("web-claude", "host", "claude", "/work/web"),
        ];
        let matched = agent_for_session(
            &declarations,
            &session(HarnessProvider::Claude, "/work/web"),
        )
        .expect("the web checkout is declared");
        assert_eq!(matched.agent_id, "web-claude");
    }

    #[test]
    fn a_trailing_separator_is_not_a_different_directory() {
        let mut declaration = AgentDeclaration::new("api-claude", "host", "claude", "");
        declaration.workspace = WorkspaceRef::checkout("/work/api/");
        let declarations = vec![declaration];
        assert!(agent_for_session(
            &declarations,
            &session(HarnessProvider::Claude, "/work/api")
        )
        .is_some());
    }

    #[test]
    fn a_different_harness_in_the_same_directory_is_a_different_agent() {
        let declarations = vec![AgentDeclaration::new(
            "api-codex",
            "host",
            "codex",
            "/work/api",
        )];
        assert!(
            agent_for_session(
                &declarations,
                &session(HarnessProvider::Claude, "/work/api")
            )
            .is_none(),
            "claude in the codex agent's directory is not that agent"
        );
    }

    #[test]
    fn an_undeclared_directory_resolves_to_no_agent() {
        let declarations = vec![AgentDeclaration::new(
            "api-claude",
            "host",
            "claude",
            "/work/api",
        )];
        assert!(agent_for_session(
            &declarations,
            &session(HarnessProvider::Claude, "/elsewhere")
        )
        .is_none());
    }
}
