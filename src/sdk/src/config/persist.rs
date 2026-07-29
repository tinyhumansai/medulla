//! Writing individual config sections back to disk.
//!
//! Config is normally read-only at runtime: it is loaded once by layering
//! several files together. A few sections are user state rather than user
//! preference, though, and the app owns them — the onboarding gate is written
//! the moment the welcome flow finishes.
//!
//! Every writer here follows the same rule: parse the target file into a TOML
//! table, replace only its own section, and write the whole document back.
//! Unrelated keys — and any other file in the layered load — are preserved.

use std::path::Path;

/// Writes a single `key` into the `[section]` table of the TOML file at `path`.
///
/// This is the generic form of the writers below: it parses the whole document,
/// merges `value` into `section` (creating either when absent), and writes the
/// document back, so unrelated keys and sections survive. Comments and key
/// ordering are *not* preserved — `toml` re-renders the document.
///
/// A missing or empty file is treated as an empty document rather than an
/// error, matching the loader. Returns an error when the file exists but cannot
/// be parsed, or when the directory or file cannot be written.
///
/// Note this writes to one specific file. When config is layered, a
/// higher-precedence file may still override what is written here; the caller
/// is responsible for telling the user which file it targeted.
pub fn persist_setting(
    path: &Path,
    section: &str,
    key: &str,
    value: toml::Value,
) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    let mut table = match doc.get(section) {
        Some(toml::Value::Table(existing)) => existing.clone(),
        _ => toml::Table::new(),
    };
    table.insert(key.to_string(), value);
    doc.insert(section.to_string(), toml::Value::Table(table));
    write_document(path, &doc)
}

/// Replace a top-level key, preserving every other key and section.
///
/// Distinct from [`persist_setting`], which writes *into* a `[section]`. An
/// array-of-tables such as `[[hosts]]` is a root-level value, and nesting it
/// under a section would silently produce a key nothing reads.
pub fn persist_root_setting(path: &Path, key: &str, value: toml::Value) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    doc.insert(key.to_string(), value);
    write_document(path, &doc)
}

/// Replace one complete TOML section while preserving every other section.
///
/// This is the reusable boundary for components such as the theme editor whose
/// values naturally form one coherent table rather than independent settings.
pub fn persist_section(path: &Path, section: &str, values: toml::Table) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    doc.insert(section.to_string(), toml::Value::Table(values));
    write_document(path, &doc)
}

/// Removes `key` from the `[section]` table, leaving the section in place.
///
/// Used to clear an optional setting back to "unset" so the layered default
/// applies again, rather than pinning it to the default's current value.
pub fn clear_setting(path: &Path, section: &str, key: &str) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    if let Some(toml::Value::Table(existing)) = doc.get(section) {
        let mut table = existing.clone();
        table.remove(key);
        doc.insert(section.to_string(), toml::Value::Table(table));
        write_document(path, &doc)?;
    }
    Ok(())
}

/// Persist the top-level `routingStrategy` key (camelCase wire value), preserving
/// every other key and section. Used by the Routing tab to remember the operator's
/// selection across runs.
pub fn persist_routing_strategy(path: &Path, wire_value: &str) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    doc.insert(
        "routingStrategy".to_string(),
        toml::Value::String(wire_value.to_string()),
    );
    write_document(path, &doc)
}

/// Persist the top-level `subscriptionRoutingStrategy` key while preserving
/// every other key and section.
pub fn persist_subscription_routing_strategy(path: &Path, wire_value: &str) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    doc.insert(
        "subscriptionRoutingStrategy".to_string(),
        toml::Value::String(wire_value.to_string()),
    );
    write_document(path, &doc)
}

/// Replace the top-level `customHarnesses` array while preserving all unrelated
/// configuration.
///
/// Presets contain only model routing and environment-variable names. The
/// referenced key value never enters this function or the resulting document.
pub fn persist_custom_harnesses(
    path: &Path,
    harnesses: &[crate::config::CustomHarnessConfig],
) -> anyhow::Result<()> {
    let mut doc = read_document(path)?;
    let value = toml::Value::try_from(harnesses)
        .map_err(|error| anyhow::anyhow!("Cannot serialize custom harnesses: {error}"))?;
    doc.insert("customHarnesses".to_string(), value);
    write_document(path, &doc)
}

/// Parse `path` into a TOML table, treating an absent file as an empty document.
fn read_document(path: &Path) -> anyhow::Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(anyhow::anyhow!("Cannot read {}: {e}", path.display())),
    }
}

/// Render `doc` and atomically write it to `path`.
fn write_document(path: &Path, doc: &toml::Table) -> anyhow::Result<()> {
    let rendered =
        toml::to_string_pretty(doc).map_err(|e| anyhow::anyhow!("Cannot serialize config: {e}"))?;
    crate::persistence::write_atomic(path, rendered.as_bytes())
        .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))
}

/// Records that the welcome flow has run, so it is not shown again.
///
/// Writes only the `[onboarding]` section of `path`, creating the file (and its
/// parent directory) when absent. A missing or empty file is treated as an empty
/// document rather than an error, matching how the loader tolerates absent
/// config. Returns an error only when the file exists but cannot be parsed or
/// written — the caller should surface that without failing the flow, since the
/// reward itself is already recorded server-side.
pub fn persist_welcome_completed(path: &Path, completed: bool) -> anyhow::Result<()> {
    persist_setting(
        path,
        "onboarding",
        "welcomeCompleted",
        toml::Value::Boolean(completed),
    )
}

/// Write the orchestrator's worker roster to the `[hub]` table.
///
/// Replaces the list wholesale rather than merging: the roster in memory is the
/// operator's current intent, and a merge would resurrect a worker they had just
/// removed. An empty roster writes an empty array, which is how "I removed the
/// last one" survives a restart.
pub fn persist_hub_workers(
    path: &Path,
    workers: &[crate::config::HubWorkerConfig],
) -> anyhow::Result<()> {
    let rows: Vec<toml::Value> = workers
        .iter()
        .map(|w| {
            let mut row = toml::Table::new();
            row.insert("id".into(), toml::Value::String(w.id.clone()));
            row.insert("address".into(), toml::Value::String(w.address.clone()));
            row.insert("harness".into(), toml::Value::String(w.harness.clone()));
            if let Some(label) = &w.label {
                row.insert("label".into(), toml::Value::String(label.clone()));
            }
            row.insert("selected".into(), toml::Value::Boolean(w.selected));
            toml::Value::Table(row)
        })
        .collect();
    persist_setting(path, "hub", "workers", toml::Value::Array(rows))
}

/// Replace the workspace roots this daemon may advertise to an orchestrator.
///
/// The list is written under `[workflow].workspaces`, which is shared with the
/// main TUI's repository view. Replacing rather than merging makes removals
/// durable and keeps the worker's permission boundary legible on disk.
pub fn persist_workflow_workspaces(path: &Path, workspaces: &[String]) -> anyhow::Result<()> {
    persist_setting(
        path,
        "workflow",
        "workspaces",
        toml::Value::Array(
            workspaces
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    )
}

/// Replace the extra hosts this machine runs (`[[hosts]]`).
///
/// Replacing rather than merging is what makes a removal durable: a merge would
/// leave a host the operator deleted still on disk, and it would bind an address
/// and start a harness again on the next launch.
///
/// Only the fields that make a host distinct are written. The rest are left to
/// the section defaults, so a config stays readable and a later change to those
/// defaults reaches hosts declared today.
pub fn persist_local_hosts(
    path: &Path,
    hosts: &[crate::config::HostSection],
) -> anyhow::Result<()> {
    let rows: Vec<toml::Value> = hosts
        .iter()
        .map(|host| {
            let mut row = toml::map::Map::new();
            if !host.name.trim().is_empty() {
                row.insert("name".into(), toml::Value::String(host.name.clone()));
            }
            // The section default is not a choice, so writing it back would
            // turn "no address, derive one" into "bind the primary's address"
            // — and every extra would collide on the next launch. Only an
            // address the operator actually picked is persisted.
            let address = host.address.trim();
            if !address.is_empty() && address != crate::config::HostSection::default().address {
                row.insert("address".into(), toml::Value::String(address.to_string()));
            }
            if !host.workspace.trim().is_empty() {
                row.insert(
                    "workspace".into(),
                    toml::Value::String(host.workspace.clone()),
                );
            }
            if !host.providers.is_empty() {
                row.insert(
                    "providers".into(),
                    toml::Value::Array(
                        host.providers
                            .iter()
                            .cloned()
                            .map(toml::Value::String)
                            .collect(),
                    ),
                );
            }
            if !host.default_provider.trim().is_empty() {
                row.insert(
                    "defaultProvider".into(),
                    toml::Value::String(host.default_provider.clone()),
                );
            }
            toml::Value::Table(row)
        })
        .collect();
    persist_root_setting(path, "hosts", toml::Value::Array(rows))
}

/// Replace the extra directories this device advertises (`[host].workspaces`).
///
/// Replacing rather than merging is what makes a removal durable: a merge would
/// leave a directory the operator deleted from the list still on disk, and it
/// would come back on the next launch.
pub fn persist_host_workspaces(path: &Path, workspaces: &[String]) -> anyhow::Result<()> {
    persist_setting(
        path,
        "host",
        "workspaces",
        toml::Value::Array(
            workspaces
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    )
}

/// Replace the static tiny.place peer roster without touching worker identity.
///
/// The daemon TUI uses this when an operator adds a master. Only public peer
/// addresses and labels are stored here; the worker's signing key remains in
/// its own `identityDir` wallet and is never copied into the orchestrator
/// configuration.
pub fn persist_tinyplace_peers(path: &Path, peers: &[crate::config::Peer]) -> anyhow::Result<()> {
    let rows = peers
        .iter()
        .map(|peer| {
            let mut row = toml::Table::new();
            row.insert("id".into(), toml::Value::String(peer.id.clone()));
            if let Some(value) = &peer.name {
                row.insert("name".into(), toml::Value::String(value.clone()));
            }
            if let Some(value) = &peer.handle {
                row.insert("handle".into(), toml::Value::String(value.clone()));
            }
            if let Some(value) = &peer.address {
                row.insert("address".into(), toml::Value::String(value.clone()));
            }
            if let Some(values) = &peer.tags {
                row.insert(
                    "tags".into(),
                    toml::Value::Array(values.iter().cloned().map(toml::Value::String).collect()),
                );
            }
            if let Some(value) = &peer.description {
                row.insert("description".into(), toml::Value::String(value.clone()));
            }
            row.insert(
                "protocol".into(),
                toml::Value::String(peer.protocol.clone()),
            );
            toml::Value::Table(row)
        })
        .collect();
    persist_setting(path, "tinyplace", "peers", toml::Value::Array(rows))
}
