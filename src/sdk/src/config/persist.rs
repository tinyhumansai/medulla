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

/// Parse `path` into a TOML table, treating an absent file as an empty document.
fn read_document(path: &Path) -> anyhow::Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(anyhow::anyhow!("Cannot read {}: {e}", path.display())),
    }
}

/// Render `doc` and write it to `path`, creating the parent directory if needed.
fn write_document(path: &Path, doc: &toml::Table) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Cannot create {}: {e}", parent.display()))?;
        }
    }
    let rendered =
        toml::to_string_pretty(doc).map_err(|e| anyhow::anyhow!("Cannot serialize config: {e}"))?;
    std::fs::write(path, rendered)
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
