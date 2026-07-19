//! Workspace initialisation: authoring a `MEDULLA.md` for a directory.
//!
//! A `MEDULLA.md` at a workspace root tells the orchestrator what that
//! directory *is* and how to route work over it — a short summary plus advisory
//! preferences (harnesses, models, routing hints). `medulla init` drafts one by
//! reading the repo's own instruction files (`AGENTS.md` / `CLAUDE.md` /
//! `README.md`) and asking a model to distil them, then writes the result for
//! the operator to review and edit.
//!
//! The module is split by responsibility: [`types`] holds the data model,
//! [`template`] renders the docs-shipped scaffold, and [`draft`] owns the model
//! call. This file wires them together and owns the filesystem edges (reading
//! sources, writing the profile, reading one back for the run request).
//!
//! Offline is a first-class path: with no API key and no backend login the
//! draft falls back to a deterministic stub, so `init` still produces a valid,
//! hand-editable file.

mod draft;
mod template;
pub mod types;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tinycortex::memory::score::extract::ChatProvider;

pub use draft::{build_user_prompt, draft_profile, parse_draft};
pub use template::render_medulla_md;
pub use types::{DraftedProfile, InitOutcome, InitSources};

/// The profile file name, at a workspace root.
pub const PROFILE_FILE: &str = "MEDULLA.md";

/// Read one optional instruction file, treating an unreadable file as absent —
/// `init` is best-effort over whatever the repo happens to have.
fn read_optional(dir: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(name)).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// Collect the instruction files `init` drafts from. Never fails: a directory
/// with none of them yields empty sources.
pub fn read_sources(dir: &Path) -> InitSources {
    InitSources {
        dir: dir.to_path_buf(),
        agents_md: read_optional(dir, "AGENTS.md"),
        claude_md: read_optional(dir, "CLAUDE.md"),
        readme_md: read_optional(dir, "README.md"),
    }
}

/// The path a workspace's profile lives at.
pub fn profile_path(dir: &Path) -> PathBuf {
    dir.join(PROFILE_FILE)
}

/// Read a workspace's `MEDULLA.md`, if it has one. Returns the verbatim text —
/// the medulla SDK owns the format, so nothing is parsed here; this is what the
/// run request forwards to the backend.
pub fn read_medulla_md(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(profile_path(dir)).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// Write the rendered profile to `<dir>/MEDULLA.md`.
///
/// Refuses to clobber an existing profile unless `force` is set — an authored
/// profile is hand-tuned operator knowledge, and silently overwriting it would
/// discard exactly the content this feature exists to preserve.
pub fn write_medulla_md(dir: &Path, contents: &str, force: bool) -> Result<PathBuf> {
    let path = profile_path(dir);
    if path.exists() && !force {
        return Err(anyhow!(
            "{} already exists — pass --force to overwrite it",
            path.display()
        ));
    }
    if !dir.exists() {
        return Err(anyhow!("{} does not exist", dir.display()));
    }
    fs::write(&path, contents)?;
    Ok(path)
}

/// Collect the run-request payload for a set of workspaces: each directory that
/// has a `MEDULLA.md` contributes one entry, verbatim. Directories without one
/// are skipped, so this is safe to call over every workspace in play.
///
/// `workspace` is the directory path as given, which must match what the roster
/// reports for an agent (`metadata.workspace`) for the profile to be attributed
/// to that agent in `agent_list`.
pub fn collect_profile_inputs(dirs: &[PathBuf]) -> Vec<crate::client::WorkspaceProfileInput> {
    dirs.iter()
        .filter_map(|dir| {
            read_medulla_md(dir).map(|medulla_md| crate::client::WorkspaceProfileInput {
                workspace: dir.display().to_string(),
                medulla_md,
            })
        })
        .collect()
}

/// Draft and write a profile, resolving the model from memory settings.
///
/// This is the entry point callers outside the SDK should use: it keeps the
/// vendor `ChatProvider` type inside this crate's boundary (per the SDK's
/// no-vendor-types-cross-the-boundary rule). `offline` skips the model call
/// entirely; otherwise an unavailable model degrades to the stub rather than
/// failing, so `init` always leaves a usable file behind.
pub async fn init_workspace_with_settings(
    dir: &Path,
    settings: &crate::memory::MemorySettings,
    offline: bool,
    force: bool,
) -> Result<InitOutcome> {
    let provider = if offline {
        None
    } else {
        crate::memory::chat_provider(settings).ok()
    };
    init_workspace(
        dir,
        provider.as_ref().map(|p| p as &dyn ChatProvider),
        force,
    )
    .await
}

/// Whether a model is reachable with these settings — lets a caller warn before
/// falling back to the stub, without naming a vendor type.
pub fn model_available(settings: &crate::memory::MemorySettings) -> bool {
    crate::memory::chat_provider(settings).is_ok()
}

/// Draft a profile for `dir` and write it.
///
/// With a `provider`, the body is drafted from the directory's instruction
/// files; without one — or when the model call fails — a deterministic stub is
/// written instead so the operator still gets a valid file to edit. The
/// returned [`InitOutcome`] reports which happened.
pub async fn init_workspace(
    dir: &Path,
    provider: Option<&dyn ChatProvider>,
    force: bool,
) -> Result<InitOutcome> {
    // Fail before spending a model call when the file is already there.
    let path = profile_path(dir);
    if path.exists() && !force {
        return Err(anyhow!(
            "{} already exists — pass --force to overwrite it",
            path.display()
        ));
    }

    let sources = read_sources(dir);
    let (draft, drafted) = match provider {
        Some(provider) if !sources.is_empty() => match draft_profile(provider, &sources).await {
            Ok(draft) => (draft, true),
            // A provider/parse failure must not lose the operator's `init`: fall
            // back to the stub and let the outcome report that it was not drafted.
            Err(_) => (DraftedProfile::stub(), false),
        },
        _ => (DraftedProfile::stub(), false),
    };

    let contents = render_medulla_md(&draft);
    let path = write_medulla_md(dir, &contents, force)?;
    Ok(InitOutcome {
        path,
        contents,
        drafted,
        sources: sources.found(),
    })
}
