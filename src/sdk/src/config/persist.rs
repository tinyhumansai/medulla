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

/// Records that the welcome flow has run, so it is not shown again.
///
/// Writes only the `[onboarding]` section of `path`, creating the file (and its
/// parent directory) when absent. A missing or empty file is treated as an empty
/// document rather than an error, matching how the loader tolerates absent
/// config. Returns an error only when the file exists but cannot be parsed or
/// written — the caller should surface that without failing the flow, since the
/// reward itself is already recorded server-side.
pub fn persist_welcome_completed(path: &Path, completed: bool) -> anyhow::Result<()> {
    use toml::Value;

    let mut doc: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Cannot parse {}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(anyhow::anyhow!("Cannot read {}: {e}", path.display())),
    };

    // Merge into any existing section so unrelated onboarding keys survive.
    let mut section = match doc.get("onboarding") {
        Some(Value::Table(existing)) => existing.clone(),
        _ => toml::Table::new(),
    };
    section.insert("welcomeCompleted".into(), Value::Boolean(completed));
    doc.insert("onboarding".into(), Value::Table(section));

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Cannot create {}: {e}", parent.display()))?;
        }
    }
    let rendered = toml::to_string_pretty(&doc)
        .map_err(|e| anyhow::anyhow!("Cannot serialize onboarding config: {e}"))?;
    std::fs::write(path, rendered)
        .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))?;
    Ok(())
}
