//! Reading, querying and writing the declared-agent list.
//!
//! The store is `[fleet].agentDeclarations` in the operator's config file —
//! the same file, loader and writer as every other section, because a second
//! store would be a second answer to "which agents exist on this machine".
//! [`crate::config::persist_agent_declarations`] does the writing; this module
//! is the read/mutate side the UI and the roster consume.
//!
//! The mutation helpers come in two grains. The pure ones
//! ([`upsert_agent_declaration`], [`remove_agent_declaration`]) edit a list in
//! memory and are what a caller holding a `FleetConfig` uses; the persisting ones
//! ([`declare_agent`], [`undeclare_agent`]) apply the same edit and write the
//! file, returning the list as it now stands so the caller can adopt it without
//! a second read. That returned list is the in-memory reload; a full layered
//! reload is [`load_config`](super::load_config), which a higher-precedence
//! config file can still override (see [`super::persist`]).

use std::path::Path;

use crate::runtime::AgentDeclaration;

/// The declarations belonging to `host_id`, in declaration order.
///
/// The local roster is exactly this list for the host that is running: one
/// [`WorkerSpec`](crate::hub::WorkerSpec) per entry.
pub fn agent_declarations_for_host<'a>(
    declarations: &'a [AgentDeclaration],
    host_id: &str,
) -> Vec<&'a AgentDeclaration> {
    declarations
        .iter()
        .filter(|declaration| declaration.on_host(host_id))
        .collect()
}

/// The declaration with this `agentId`, if any.
pub fn agent_declaration<'a>(
    declarations: &'a [AgentDeclaration],
    agent_id: &str,
) -> Option<&'a AgentDeclaration> {
    let wanted = agent_id.trim();
    declarations
        .iter()
        .find(|declaration| declaration.agent_id.trim() == wanted)
}

/// Every declared `agentId`, for minting one that does not collide.
pub fn declared_agent_ids(declarations: &[AgentDeclaration]) -> Vec<String> {
    declarations
        .iter()
        .map(|declaration| declaration.agent_id.clone())
        .collect()
}

/// Insert `incoming`, or replace the declaration that already has its id.
///
/// Keyed on `agentId` alone: the id is what a dispatch targets, so two entries
/// sharing one would make the target ambiguous, while the same agent moving to
/// another harness or directory is an edit rather than a new agent. Returns
/// `true` when this added an agent, `false` when it updated one — the caller
/// needs the difference to narrate it.
///
/// An in-place replacement keeps its position in the list, so editing an agent
/// does not reshuffle the roster the operator is looking at.
pub fn upsert_agent_declaration(
    declarations: &mut Vec<AgentDeclaration>,
    incoming: AgentDeclaration,
) -> bool {
    match declarations
        .iter()
        .position(|held| held.agent_id.trim() == incoming.agent_id.trim())
    {
        Some(index) => {
            declarations[index] = incoming;
            false
        }
        None => {
            declarations.push(incoming);
            true
        }
    }
}

/// Remove the declaration with `agent_id`, returning it when there was one.
pub fn remove_agent_declaration(
    declarations: &mut Vec<AgentDeclaration>,
    agent_id: &str,
) -> Option<AgentDeclaration> {
    let wanted = agent_id.trim();
    let index = declarations
        .iter()
        .position(|held| held.agent_id.trim() == wanted)?;
    Some(declarations.remove(index))
}

/// Declare (or redeclare) an agent and write the list to `path`.
///
/// `current` is the list as loaded; the result is the list as written, which the
/// caller assigns back into its `FleetConfig` so on-screen state and the file
/// agree. Nothing else in the file is touched.
///
/// # Errors
///
/// Returns an error when the config file cannot be parsed or written. The
/// in-memory edit is *not* applied in that case — the returned list is the only
/// way to adopt it — so a failed write cannot leave the UI showing an agent that
/// will not survive a restart.
pub fn declare_agent(
    path: &Path,
    current: &[AgentDeclaration],
    incoming: AgentDeclaration,
) -> anyhow::Result<Vec<AgentDeclaration>> {
    let mut declarations = current.to_vec();
    upsert_agent_declaration(&mut declarations, incoming);
    super::persist_agent_declarations(path, &declarations)?;
    Ok(declarations)
}

/// Remove an agent declaration and write the list to `path`.
///
/// Removing only undeclares: the workspace directory and anything in it are left
/// alone, because the operator asked the orchestrator to stop placing work
/// there, not to lose their checkout.
///
/// # Errors
///
/// Errors when `agent_id` is not declared, or when the config file cannot be
/// parsed or written.
pub fn undeclare_agent(
    path: &Path,
    current: &[AgentDeclaration],
    agent_id: &str,
) -> anyhow::Result<Vec<AgentDeclaration>> {
    let mut declarations = current.to_vec();
    if remove_agent_declaration(&mut declarations, agent_id).is_none() {
        anyhow::bail!("no agent \"{agent_id}\" is declared");
    }
    super::persist_agent_declarations(path, &declarations)?;
    Ok(declarations)
}

/// The declarations recorded in one config file.
///
/// Reads that single file rather than the layered load, which is what makes it
/// the honest read-back of [`declare_agent`]: it answers "what is written where
/// I wrote it". A missing, empty or unparseable file yields an empty list —
/// nothing declared — because a declaration store that fails loudly at read time
/// would take the whole roster down with it.
pub fn load_agent_declarations(path: &Path) -> Vec<AgentDeclaration> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let parsed = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
    {
        toml::from_str::<super::TuiConfig>(&text).ok()
    } else {
        serde_json::from_str::<super::TuiConfig>(&text).ok()
    };
    parsed
        .map(|config| config.fleet.agent_declarations)
        .unwrap_or_default()
}
