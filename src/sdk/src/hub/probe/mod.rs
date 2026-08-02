//! Building the `capabilities` payload the backend records for a worker.
//!
//! The orchestrator's picture of a worker is whatever lands here. `agent_list`
//! and `agent_capabilities` read it, and it is the only place the reasoning tier
//! learns what a machine actually has — which directories it can work in, what
//! the project there is, which tools and MCP servers are wired up.
//!
//! The worker already reports all of that: the daemon's capability probe asks
//! the harness itself and returns an [`AgentCapabilities`] over the encrypted
//! relay. Until now the hub answered the backend with two static facts — the
//! configured harness name and the synthetic string `"<harness> daemon"` — and
//! dropped everything the probe had gone and fetched. So an orchestrator asking
//! "what can this agent do?" was told "claude daemon", and delegated work with
//! no idea what was in the directory.
//!
//! [`capabilities_payload`] carries the probe through instead. It emits exactly
//! the keys the backend's `sanitizeCapabilities` keeps, so nothing here depends
//! on the backend learning a new shape.

use serde_json::{json, Map, Value};

use crate::tinyplace::AgentCapabilities;

/// The union of the toggled roles' tool allowlists.
///
/// `None` means unconstrained, and must stay distinct from `Some(vec![])` —
/// "allowed nothing" — or an unspecified worker would advertise no tools at all.
/// A worker names no roles, or names only roles that declare no allowlist of
/// their own, and both mean unconstrained.
///
/// # Fail-closed on unresolvable roles
///
/// A worker that names roles and resolves *none* of them is misconfigured, and
/// the permissive reading of that is the worst one available: a typo in a role
/// id would silently advertise every tool the harness has, which is exactly the
/// over-claim the allowlist exists to prevent. It denies instead — an agent
/// advertising no tools is visibly broken and gets fixed, whereas one
/// advertising too many looks fine and routes wrong.
///
/// Only when there is a catalog to judge against. An empty one means the
/// templates have not been read, not that every id is bad, and failing closed
/// there would disable a whole fleet on a transient.
pub(super) fn role_tool_allowlist(
    roles: &[String],
    catalog: &[crate::runtime::AgentTemplate],
) -> Option<Vec<String>> {
    let mut allowed: Vec<String> = Vec::new();
    let mut constrained = false;
    let mut resolved = 0usize;
    for role in roles
        .iter()
        .filter_map(|id| catalog.iter().find(|t| &t.id == id))
    {
        resolved += 1;
        if let Some(tools) = &role.tools {
            constrained = true;
            for tool in tools {
                if !allowed.iter().any(|held| held == tool) {
                    allowed.push(tool.clone());
                }
            }
        }
    }
    if !roles.is_empty() && resolved == 0 && !catalog.is_empty() {
        constrained = true;
    }
    constrained.then_some(allowed)
}

/// The backend-shaped `capabilities` bag for one worker.
///
/// `harness` is the roster's configured harness name, used when the probe
/// reported no providers of its own. `caps` is the probe result, or `None` when
/// the worker could not be reached — the probe fails open, so an unreachable
/// worker still advertises the static facts rather than nothing at all.
///
/// Empty values are omitted rather than sent blank: the backend drops empty
/// strings and arrays anyway, and an absent key is the honest encoding of "not
/// reported" where `""` reads as a measurement someone took.
pub(super) fn capabilities_payload(
    harness: &str,
    caps: Option<&AgentCapabilities>,
    allowed_tools: Option<&[String]>,
    roles: &[String],
) -> Value {
    let harness = harness.trim();
    let mut obj = Map::new();

    // The roles this worker is offered for, so an orchestrator that asks what
    // an agent can do is told what it is *for* as well as what it *has*.
    // Omitted when empty, like every other value here: an absent key reads as
    // "not specified", where `[]` reads as "deliberately none".
    //
    // Whether the backend keeps this depends on its `sanitizeCapabilities`,
    // which drops keys it does not know. The descriptor carries the same roles
    // through `description` and `tags`, which are certainly preserved — so this
    // is completeness, not the mechanism anything relies on.
    if !roles.is_empty() {
        obj.insert("roles".to_string(), json!(roles));
    }

    // Providers: what the probe found installed, else the one the roster names.
    let providers: Vec<String> = match caps {
        Some(caps) if !caps.providers.is_empty() => caps
            .providers
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        _ => harness
            .is_empty()
            .then(Vec::new)
            .unwrap_or_else(|| vec![harness.to_string()]),
    };
    obj.insert("providers".to_string(), json!(providers));

    // The summary is the single most useful field: the harness's own account of
    // what the project is and what can be done in it. The synthetic
    // "<harness> daemon" is a placeholder for when there is no real one, not
    // something to prefer over it.
    let summary = caps
        .and_then(|c| c.summary.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| (!harness.is_empty()).then(|| format!("{harness} daemon")));
    if let Some(summary) = summary {
        obj.insert("summary".to_string(), json!(summary));
    }

    if let Some(caps) = caps {
        for (key, value) in [
            ("cwd", caps.cwd.as_deref()),
            ("project", caps.project.as_deref()),
            ("branch", caps.branch.as_deref()),
        ] {
            if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
                obj.insert(key.to_string(), json!(value));
            }
        }
        for (key, values) in [
            ("accessibleDirs", &caps.accessible_dirs),
            ("tools", &caps.tools),
            ("mcpServers", &caps.mcp_servers),
        ] {
            let mut values: Vec<&str> = values
                .iter()
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .collect();
            // The probe reports what the *harness* has; a role says what this
            // worker is *allowed*. Reporting the first unfiltered makes the two
            // channels contradict each other about the same worker — the
            // descriptor saying "reviews diffs" while capabilities claim it can
            // edit files — and capabilities is the one the reasoning tier reads
            // for "what can this agent do?".
            //
            // Applied here rather than asked of the probe: the allowlist is
            // known locally and exactly, whereas telling a harness to limit
            // itself is a request it may or may not honour, and would cost a
            // fresh run every time a role changed.
            if key == "tools" {
                if let Some(allowed) = allowed_tools {
                    values.retain(|tool| allowed.iter().any(|a| a.eq_ignore_ascii_case(tool)));
                }
            }
            if !values.is_empty() {
                obj.insert(key.to_string(), json!(values));
            }
        }
        if !caps.custom_harnesses.is_empty() {
            obj.insert(
                "customHarnesses".to_string(),
                serde_json::to_value(&caps.custom_harnesses).unwrap_or_default(),
            );
        }
    }

    let mut payload = Value::Object(obj);
    if let Some(caps) = caps {
        decorate_with_budgets(&mut payload, caps);
    }
    payload
}

/// Map a worker's [`AgentCapabilities`] budgets/readiness onto the backend-shaped
/// `capabilities` payload the backend's `sanitizeCapabilities` reads.
///
/// The worker frame carries a per-provider `readiness` array and a `budgets`
/// array; the backend's `AgentCapabilities` bag carries a flat `harnessBudgets`
/// array plus a single advisory `ready`/`readyReason` pair. The `HarnessBudget`
/// entries already serialize in the camelCase the backend's `parseHarnessBudget`
/// accepts, so `harnessBudgets` is the budget array verbatim. `ready` is the AND
/// of the readiness entries (usable only when every installed harness is), and
/// `readyReason` surfaces the first not-ready reason.
pub(crate) fn decorate_with_budgets(capabilities: &mut Value, caps: &AgentCapabilities) {
    let Some(obj) = capabilities.as_object_mut() else {
        return;
    };
    if !caps.budgets.is_empty() {
        if let Ok(budgets) = serde_json::to_value(&caps.budgets) {
            obj.insert("harnessBudgets".to_string(), budgets);
        }
    }
    if !caps.readiness.is_empty() {
        let not_ready = caps.readiness.iter().find(|r| !r.ready);
        obj.insert("ready".to_string(), Value::Bool(not_ready.is_none()));
        if let Some(reason) = not_ready.and_then(|r| r.reason.clone()) {
            obj.insert("readyReason".to_string(), Value::String(reason));
        }
    }
}

#[cfg(test)]
mod tests;
