//! Workspace initialisation: registering a directory and authoring its
//! `MEDULLA.md`.
//!
//! Two things are needed before an orchestrator can work in a directory, and
//! this module owns both — `medulla init` does the first, `medulla workspace
//! add` does both:
//!
//! 1. **Describe it.** A `MEDULLA.md` at the workspace root says what the
//!    directory *is*, which files it is made of, and how to route work over it —
//!    a short summary, a scanned file layout, and advisory preferences
//!    (harnesses, models, routing hints). The summary is a deterministic stub
//!    for the operator to edit — the model-drafted variant went out with the
//!    memory layer that owned the provider seam.
//! 2. **Register it.** A profile nothing knows about is inert, so the directory
//!    is also enrolled in the operator's config: `[workflow].workspaces`, whose
//!    profiles ride every backend session mint, and `[fleet].workspaces`, the
//!    declared chain the orchestrator places work onto. See [`registry`].
//!
//! The module is split by responsibility: [`types`] holds the data model,
//! `layout` scans the tree, `template` renders the docs-shipped scaffold, and
//! [`registry`] owns the config writes. This file wires them together and owns
//! the filesystem edges (reading sources, writing the profile, reading one back
//! for the run request).
//!
//! Everything here is offline: the profile body is a stub, and the layout scan
//! and registration never needed a model in the first place.

mod layout;
pub mod registry;
mod template;
pub mod types;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub use layout::{layout_block, scan_layout};
pub use registry::{deregister_workspace, register_workspace, workspace_id, WorkspaceRegistration};
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

/// Ensure a workspace has a profile, then register it — the whole of what
/// `medulla workspace add` does, in one call.
///
/// An **existing** `MEDULLA.md` is kept, not an error: registration is about
/// enrolling the directory, and the overwhelmingly common paths into this
/// function — re-running `add` to refresh an entry, or running it after
/// `medulla init` — both start with a profile already on disk. Refusing them
/// would make registering an already-described workspace impossible except by
/// destroying the description first. `force` redrafts instead of keeping.
///
/// Otherwise the profile is written first: registration points at a path, and a
/// registry entry whose `MEDULLA.md` failed to write would advertise a workspace
/// the orchestrator can find but not read. A registration failure is *not* fatal
/// to the profile — the file is already on disk and useful — so it is reported on
/// [`InitOutcome::registration_error`] rather than returned as an error, and the
/// caller decides how loudly to say so.
///
/// # Errors
///
/// Returns an error only when a profile is needed and cannot be drafted or
/// written (see [`init_workspace`]).
pub async fn init_and_register_workspace(
    dir: &Path,
    config_path: &Path,
    config: &crate::config::TuiConfig,
    harness: Option<&str>,
    force: bool,
) -> Result<InitOutcome> {
    let mut outcome = match read_medulla_md(dir) {
        Some(contents) if !force => InitOutcome {
            path: profile_path(dir),
            contents,
            drafted: false,
            sources: Vec::new(),
            // Reported from the file that is actually on disk, so a kept profile
            // is described by its own layout rather than a fresh scan the
            // operator never asked for.
            layout: Vec::new(),
            kept_profile: true,
            registration: None,
            registration_error: None,
        },
        _ => init_workspace(dir, force).await?,
    };
    match register_workspace(config_path, config, dir, harness) {
        Ok(registration) => outcome.registration = Some(registration),
        Err(err) => outcome.registration_error = Some(err.to_string()),
    }
    Ok(outcome)
}

/// Draft a profile for `dir` and write it.
///
/// The body is a deterministic stub for the operator to edit; the scanned
/// layout is the part that carries real information. [`InitOutcome::drafted`]
/// is therefore always false — it stays on the type because a drafted body is
/// what returns when the model seam does.
pub async fn init_workspace(dir: &Path, force: bool) -> Result<InitOutcome> {
    // Fail before spending a model call when the file is already there.
    let path = profile_path(dir);
    if path.exists() && !force {
        return Err(anyhow!(
            "{} already exists — pass --force to overwrite it",
            path.display()
        ));
    }

    let sources = read_sources(dir);
    // The layout is the part of the profile that carries real information, and
    // it is read straight off the tree.
    let layout = scan_layout(dir);
    let contents = render_medulla_md(&DraftedProfile::stub(), &layout);
    let path = write_medulla_md(dir, &contents, force)?;
    Ok(InitOutcome {
        path,
        contents,
        drafted: false,
        sources: sources.found(),
        layout,
        kept_profile: false,
        registration: None,
        registration_error: None,
    })
}
