//! The on-disk template store: `.medulla/agents/*.toml`, one template per file.
//!
//! Reading is forgiving by design. A directory that does not exist is not an
//! error, an unreadable or malformed file costs only that template, and a file
//! that omits `id` is named by its filename — because an operator editing a
//! catalog by hand should lose the file they broke, not the nine that are fine.
//! What went wrong travels back in [`TemplateStore::errors`] so a surface can
//! say so rather than silently rendering a short list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::home::medulla_home;
use crate::runtime::fleet::{AgentTemplate, CapacitySnapshot};

/// What a read of the template directories found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateStore {
    /// Templates in load order, later directories having replaced earlier ones
    /// of the same id.
    pub templates: Vec<AgentTemplate>,
    /// Directories that existed and were read, in precedence order.
    pub dirs: Vec<PathBuf>,
    /// One message per file that could not be read or parsed.
    pub errors: Vec<String>,
}

impl TemplateStore {
    /// Whether any file in any directory produced a template. A store with none
    /// is the signal to fall back to the built-in catalog.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// The template directories, lowest precedence first: the user-global
/// `<medulla home>/agents`, then the project-local `<cwd>/.medulla/agents`.
///
/// The two are always distinct directories. They used to be able to collapse
/// into one — under `MEDULLA_DEV=1` the home *was* `./.medulla`, and reading it
/// twice made every template shadow itself — but the home is now the account
/// directory one level inside the root (`./.medulla/<account>`), which no
/// project store can name.
pub fn template_dirs(env: &HashMap<String, String>, cwd: &Path) -> Vec<PathBuf> {
    vec![
        medulla_home(env).join("agents"),
        cwd.join(".medulla").join("agents"),
    ]
}

/// Read every `*.toml` in `dirs`, later directories overriding earlier ones by
/// template id.
///
/// Files within one directory are read in sorted order so the catalog is stable
/// across platforms. Never fails: a missing directory yields nothing and a bad
/// file yields an entry in [`TemplateStore::errors`].
pub fn load_templates(dirs: &[PathBuf]) -> TemplateStore {
    let mut store = TemplateStore::default();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // Not existing is the normal state, not a failure worth reporting.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                store.errors.push(format!("{}: {err}", dir.display()));
                continue;
            }
        };
        store.dirs.push(dir.clone());

        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| is_toml(path))
            .collect();
        paths.sort();

        for path in paths {
            match read_template(&path) {
                Ok(template) => upsert(&mut store.templates, template),
                Err(err) => store.errors.push(err),
            }
        }
    }
    store
}

/// Whether a path is a file this store reads.
fn is_toml(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("toml"))
            .unwrap_or(false)
}

/// Read and parse one template file, naming errors by path.
fn read_template(path: &Path) -> Result<AgentTemplate, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    parse_template(&text, stem).map_err(|err| format!("{}: {err}", path.display()))
}

/// Parse one template document.
///
/// `id` defaults to `id_fallback` — the filename, for a file — and
/// `description` to empty, so the smallest useful document is a description and
/// some instructions. Every other field is optional already.
///
/// Shared with the built-in catalog, which is the same TOML compiled in: one
/// parser means a document that works on disk works as a default and vice
/// versa.
pub(super) fn parse_template(text: &str, id_fallback: &str) -> Result<AgentTemplate, String> {
    let parsed: toml::Value = toml::from_str(text).map_err(|err| format!("invalid TOML: {err}"))?;
    let mut value: Value =
        serde_json::to_value(parsed).map_err(|err| format!("invalid TOML: {err}"))?;

    // A TOML document is always a table, so this is where the two optional
    // fields get their defaults; anything else `from_value` will explain itself.
    if let Some(object) = value.as_object_mut() {
        object
            .entry("id")
            .or_insert_with(|| Value::String(id_fallback.to_string()));
        object
            .entry("description")
            .or_insert_with(|| Value::String(String::new()));
    }

    serde_json::from_value(value).map_err(|err| err.to_string())
}

/// Add `template` to `templates`, replacing any entry with the same id in place
/// so a project-local override keeps the position of what it overrides.
fn upsert(templates: &mut Vec<AgentTemplate>, template: AgentTemplate) {
    match templates.iter_mut().find(|t| t.id == template.id) {
        Some(existing) => *existing = template,
        None => templates.push(template),
    }
}

/// `capacity` plus every template in `extra` whose id it does not already
/// declare.
///
/// Additive and never destructive: the capacity's own record wins on a
/// collision, because it comes from whatever is authoritative about the fleet,
/// and this is only what the local client can offer.
pub fn merge_templates(capacity: &CapacitySnapshot, extra: &[AgentTemplate]) -> CapacitySnapshot {
    let mut out = capacity.clone();
    for template in extra {
        if out.templates.iter().any(|t| t.id == template.id) {
            continue;
        }
        out.templates.push(template.clone());
    }
    out
}
