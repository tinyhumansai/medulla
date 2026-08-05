//! Deriving a Codex model-catalog entry for a routed model.
//!
//! Codex will not describe a model it has no catalog entry for, and the entry
//! carries more than a name: the context window, the tool shapes, and the system
//! instructions Codex sends. Vendoring a copy of somebody's catalog would freeze
//! those instructions at whatever Codex version they were written against, so
//! the entry is derived instead from the catalog the installed Codex has already
//! cached for itself (`$CODEX_HOME/models_cache.json`). It therefore always
//! matches the Codex on this machine.
//!
//! Three fields of the template are turned off, and each one is a hard 400 from
//! providers that accept only `function` tools rather than a preference:
//!
//! - `apply_patch_tool_type` — freeform `apply_patch` is sent as a `custom` tool
//! - `tool_mode` — `code_mode_only` is likewise a `custom` tool
//! - `supports_search_tool` — sent as a `tool_search` tool
//!
//! `shell_type` is deliberately left as the template has it: the richer
//! `shell_command` tool is a plain function and works fine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::types::CodexOverridesError;

/// The cached catalog Codex writes for itself, relative to its home.
const CACHE_FILE: &str = "models_cache.json";

/// Where the derived catalog for `model` is written.
///
/// Under Medulla's own state directory rather than Codex's home: this is
/// generated data belonging to a Medulla run, and writing into `$CODEX_HOME`
/// would put Medulla's file next to the operator's own config.
pub fn catalog_path(model: &str, env: &HashMap<String, String>) -> PathBuf {
    medulla_state_dir(env)
        .join("codex-catalogs")
        .join(format!("{}.json", slug_file_name(model)))
}

/// Derive and write the catalog for `model`, returning the file to hand Codex.
///
/// Rebuilt on every spawn rather than cached: it is one pass over a small file,
/// and it means a Codex upgrade never leaves a stale system prompt behind.
///
/// # Errors
///
/// Fails when Codex has never cached a catalog (the operator has not run it
/// yet), when that cache holds no usable template, or when the derived file
/// cannot be written. Each error names what to do about it.
pub fn write_catalog(
    model: &str,
    display_name: &str,
    context_window: u32,
    env: &HashMap<String, String>,
) -> Result<PathBuf, CodexOverridesError> {
    let source = codex_home(env).join(CACHE_FILE);
    let raw =
        std::fs::read_to_string(&source).map_err(|error| CodexOverridesError::CacheMissing {
            path: source.clone(),
            source: error,
        })?;
    let cached: Value =
        serde_json::from_str(&raw).map_err(|error| CodexOverridesError::CacheInvalidJson {
            path: source.clone(),
            source: error,
        })?;
    let entry = derive_entry(&cached, model, display_name, context_window)
        .ok_or(CodexOverridesError::NoUsableTemplate { path: source })?;

    let path = catalog_path(model, env);
    // `path`'s own directory (`codex-catalogs/`) always has a parent — it is
    // joined onto `medulla_state_dir`/`home_dir`, which never returns a bare
    // root — so this can only fail if the directory could not be created.
    let parent = path.parent().unwrap_or(&path);
    std::fs::create_dir_all(parent).map_err(|error| CodexOverridesError::CreateDir {
        path: parent.to_path_buf(),
        source: error,
    })?;
    let document = serde_json::to_vec_pretty(&json!({ "models": [entry] }))
        .map_err(|error| CodexOverridesError::Encode { source: error })?;
    // Written through a temporary file so a spawn that races another one never
    // hands Codex a half-written catalog. The temporary name is unique per
    // writer (pid + a random suffix): two concurrent spawns for the same model
    // otherwise derive the same deterministic `*.json.tmp` name, and whichever
    // renames second finds its own temporary file already gone.
    let temporary = parent.join(format!(
        "{}.{}-{:x}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("catalog.json"),
        std::process::id(),
        random_suffix(),
    ));
    std::fs::write(&temporary, &document).map_err(|error| CodexOverridesError::Write {
        path: temporary.clone(),
        source: error,
    })?;
    std::fs::rename(&temporary, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        CodexOverridesError::Rename {
            path: path.clone(),
            source: error,
        }
    })?;
    Ok(path)
}

/// A cheap per-call random value for the temporary catalog file name.
///
/// Not cryptographic — it only needs to make two writers racing at the same
/// instant pick different names, and the process id already tells apart
/// writers started at different times. Seeded from the address of a
/// stack-local, which varies across calls and threads without pulling in a
/// `rand` dependency for one non-cryptographic use.
fn random_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let marker = 0u8;
    let address = std::ptr::addr_of!(marker) as u64;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default() as u64;
    address ^ nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Build the catalog entry for `model` from Codex's own cached catalog.
///
/// The highest-priority real model is the template. Codex's auto-review entry is
/// excluded: it is a special-purpose profile carrying review instructions, not a
/// general coding one.
fn derive_entry(
    cached: &Value,
    model: &str,
    display_name: &str,
    context_window: u32,
) -> Option<Value> {
    let models = cached
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| cached.as_array())?;
    let template = models
        .iter()
        .filter(|entry| {
            !entry
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("auto-review")
        })
        .min_by_key(|entry| {
            entry
                .get("priority")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX)
        })?;
    let mut entry: Map<String, Value> = template.as_object()?.clone();

    entry.insert("slug".into(), json!(model));
    entry.insert("display_name".into(), json!(display_name));
    entry.insert(
        "description".into(),
        json!(format!("{display_name}, routed through Codex by Medulla.")),
    );
    entry.insert("context_window".into(), json!(context_window));
    entry.insert("max_context_window".into(), json!(context_window));
    entry.insert("effective_context_window_percent".into(), json!(95));
    entry.insert("default_reasoning_level".into(), json!("high"));
    // The routed model is the only one in this catalog, so it is the one Codex
    // must pick, and it must not offer an upgrade to a model that is not there.
    entry.insert("priority".into(), json!(1));
    entry.insert("upgrade".into(), Value::Null);
    entry.insert("availability_nux".into(), Value::Null);
    // The three tool shapes a `function`-only provider rejects — see the module
    // docs for why each one is fatal rather than merely unused.
    entry.insert("apply_patch_tool_type".into(), Value::Null);
    entry.insert("tool_mode".into(), Value::Null);
    entry.insert("supports_search_tool".into(), json!(false));
    Some(Value::Object(entry))
}

/// Codex's configuration home: `$CODEX_HOME`, else `~/.codex`.
fn codex_home(env: &HashMap<String, String>) -> PathBuf {
    if let Some(home) = env
        .get("CODEX_HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(home);
    }
    home_dir(env).join(".codex")
}

/// Medulla's state directory: `$MEDULLA_HOME`, else `~/.medulla`.
///
/// Resolved from the run's own environment rather than the process's, so a
/// harness spawned with a scratch `MEDULLA_HOME` writes its catalog there too.
fn medulla_state_dir(env: &HashMap<String, String>) -> PathBuf {
    if let Some(home) = env
        .get("MEDULLA_HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(home);
    }
    home_dir(env).join(".medulla")
}

/// The run's home directory, falling back to the current directory when a
/// stripped environment carries no `HOME`.
fn home_dir(env: &HashMap<String, String>) -> PathBuf {
    env.get("HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

/// A model slug reduced to a safe single file name.
///
/// Every OpenRouter slug contains a `/`, which would otherwise turn the file
/// name into a directory path.
fn slug_file_name(model: &str) -> String {
    let name: String = model
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "model".to_string()
    } else {
        name
    }
}
