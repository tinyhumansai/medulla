//! Build safe roster entries from a local host's agent declarations.

use std::collections::HashSet;

use medulla::daemon::embedded::{resolve_workspace, EmbeddedDaemon};
use medulla::hub::WorkerSpec;
use medulla::runtime::{seed_declarations, AgentDeclaration};

/// Turn this host's declarations into advertised worker specs and placement
/// problems without allowing a fallback to impersonate a dropped declaration.
pub(super) fn specs_for(
    daemon: &EmbeddedDaemon,
    name: &str,
    declared: &[AgentDeclaration],
) -> (Vec<WorkerSpec>, Vec<String>) {
    let host_id = daemon.address();
    let mine: Vec<_> = declared
        .iter()
        .filter(|declaration| declaration.on_host(host_id))
        .cloned()
        .collect();
    if mine.is_empty() {
        return (
            specs_from(seed_for(daemon, &HashSet::new()), daemon, name),
            Vec::new(),
        );
    }

    let mut problems = Vec::new();
    let mut dropped_ids = HashSet::new();
    let mine: Vec<_> = mine
        .into_iter()
        .filter(
            |declaration| match check_placement(declaration, host_id, daemon.workspace()) {
                Ok(()) => true,
                Err(problem) => {
                    dropped_ids.insert(declaration.agent_id.clone());
                    problems.push(problem);
                    false
                }
            },
        )
        .collect();
    let mine = if mine.is_empty() {
        seed_for(daemon, &dropped_ids)
    } else {
        mine
    };
    (specs_from(mine, daemon, name), problems)
}

/// Convert valid declarations to the roster format consumed by the hub.
fn specs_from(
    declarations: Vec<AgentDeclaration>,
    daemon: &EmbeddedDaemon,
    name: &str,
) -> Vec<WorkerSpec> {
    let single = declarations.len() == 1;
    let workspace_path = daemon.workspace().to_string();
    declarations
        .iter()
        .map(|declaration| {
            let label = declaration.name.clone().unwrap_or_else(|| {
                if single {
                    name.to_string()
                } else if shares_harness(&declarations, declaration) {
                    format!("{name} · {}", declaration.agent_id)
                } else {
                    format!("{name} · {}", declaration.harness)
                }
            });
            let mut workspace = declaration.workspace.clone();
            workspace.path = workspace_path.clone();
            WorkerSpec {
                id: declaration.agent_id.clone(),
                host_id: daemon.address().to_string(),
                address: daemon.address().to_string(),
                name: label,
                description: format!(
                    "{} on this machine · {}",
                    declaration.harness, workspace.path
                ),
                harness: declaration.harness.clone(),
                workspace: Some(workspace),
                max_sessions: declaration.max_sessions(),
            }
        })
        .collect()
}

/// Whether another declaration uses the same provider.
fn shares_harness(declarations: &[AgentDeclaration], declaration: &AgentDeclaration) -> bool {
    declarations
        .iter()
        .filter(|sibling| sibling.harness == declaration.harness)
        .count()
        > 1
}

/// Reject a declaration that names a workspace other than its host's.
fn check_placement(
    declaration: &AgentDeclaration,
    host_id: &str,
    host_workspace: &str,
) -> Result<(), String> {
    let Some(declared) = declaration.workspace.path() else {
        return Ok(());
    };
    if resolve_workspace(declared) == host_workspace {
        return Ok(());
    }
    Err(format!(
        "agent \"{}\" is declared in {declared}, but host \"{host_id}\" runs tasks in \
         {host_workspace} — every agent on a host works in the host's directory, so declare a \
         [[hosts]] entry for {declared} and put this agent on it",
        declaration.agent_id,
    ))
}

/// Seed detected providers, assigning a distinct fallback ID on collisions.
fn seed_for(daemon: &EmbeddedDaemon, forbidden_ids: &HashSet<String>) -> Vec<AgentDeclaration> {
    let harnesses: Vec<_> = daemon
        .providers()
        .iter()
        .map(|provider| provider.as_str())
        .collect();
    let mut used_ids = forbidden_ids.clone();
    seed_declarations(
        daemon.address(),
        daemon.workspace(),
        &harnesses,
        daemon.default_provider().as_str(),
    )
    .into_iter()
    .map(|mut declaration| {
        let original = declaration.agent_id.clone();
        let mut suffix = 0;
        while !used_ids.insert(declaration.agent_id.clone()) {
            suffix += 1;
            declaration.agent_id = format!("{original}-fallback-{suffix}");
        }
        declaration
    })
    .collect()
}
