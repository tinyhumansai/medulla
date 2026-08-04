//! Resolving a session to the agent that owns it, and an agent to its host.
//!
//! Two directions, one rule each, kept apart from the assembly in
//! [`super`] so both can be tested without building an [`App`](super::super::types::App).
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
//! **Follow-up (host level only).** The Hosts tab grew a shared `Host → Agent`
//! projection (`medulla::ui::hosts::host_rows`, with device-local resolution in
//! `medulla::config::local_hosts`) after this branch was cut, and it is not on
//! this tree yet. Once the two are merged, [`has_remote_host`] and
//! [`host_label`] here — and the agent level in [`super::App::rail_rows`] —
//! should source from it, so the two lenses can never disagree about what
//! exists. Session rows stay here: they are the Agents tab's own level.
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

/// Whether the rail should draw host headers at all.
///
/// Progressive disclosure: with only the local host, agents sit at the top level
/// and a permanent `mac-studio ▸` wrapper would add a level of nesting to the
/// surface an operator uses most. A single *unknown* host id does not count as a
/// remote — an agent nothing places is drawn beside the local ones rather than
/// conjuring a second machine out of a missing field.
pub fn has_remote_host<'a>(host_ids: impl IntoIterator<Item = &'a str>, local: &str) -> bool {
    let local = local.trim();
    host_ids
        .into_iter()
        .map(str::trim)
        .any(|host_id| !host_id.is_empty() && host_id != local)
}

/// The display label for a host row.
///
/// The local machine says so in words; a remote one is named by its address,
/// shortened when the address is a raw key — a 44-character base58 public key is
/// the widest thing the rail would ever hold.
pub fn host_label(host_id: &str, local: bool) -> String {
    if local {
        return "this device".to_string();
    }
    let host_id = host_id.trim();
    if host_id.is_empty() {
        "unplaced".to_string()
    } else {
        crate::ui::util::short_if_address(host_id)
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

    #[test]
    fn only_a_second_named_host_counts_as_remote() {
        assert!(!has_remote_host(["local", "local", ""], "local"));
        assert!(has_remote_host(["local", "studio"], "local"));
        assert!(
            !has_remote_host(["", ""], "local"),
            "an unplaced agent is not a machine of its own"
        );
    }

    #[test]
    fn the_local_host_says_so_and_a_remote_one_is_named() {
        assert_eq!(host_label("anything", true), "this device");
        assert_eq!(host_label("studio", false), "studio");
        assert_eq!(host_label("  ", false), "unplaced");
    }
}
