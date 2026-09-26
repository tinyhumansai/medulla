//! `medulla workspace` — the workspace registry as a CLI verb.
//!
//! A `MEDULLA.md` describes a directory; the registry is what makes the
//! orchestrator aware of it. `medulla init` writes the file and stops, so this
//! command owns the other half:
//!
//! - `add [dir]` — author the profile *and* enrol the directory, which is what
//!   an operator setting up a new repository actually wants.
//! - `list` — what the orchestrator currently knows about.
//! - `remove <dir|id>` — stop placing work there, without touching the files.
//!
//! Every action reads the layered config and writes to a single file (the same
//! one the TUI would write), so `medulla workspace` and the running app always
//! agree about what is registered.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use medulla::config::{load_config, LoadedConfig};
use medulla::init::{deregister_workspace, InitOutcome, WorkspaceRegistration};
use medulla_tui::cli::{parse_workspace_args, WorkspaceAction, WorkspaceArgs};

/// Run `medulla workspace <add|list|remove>`.
///
/// # Errors
///
/// Returns an error when config cannot be loaded, when `add` cannot write the
/// profile, or when `remove` matches no registered workspace.
pub(crate) async fn run_workspace(args: &[String]) -> anyhow::Result<()> {
    let parsed = parse_workspace_args(args);
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    let config_path = registry_config_path(parsed.config.as_deref(), &loaded, &env);

    match &parsed.action {
        WorkspaceAction::Add(dir) => {
            let target = dir.as_ref().map_or_else(|| cwd.clone(), |d| cwd.join(d));
            add(&parsed, &loaded, &config_path, &target).await
        }
        WorkspaceAction::List => {
            list(&loaded, &config_path, parsed.json);
            Ok(())
        }
        WorkspaceAction::Remove(selector) => {
            let removed = deregister_workspace(&config_path, &loaded.config, selector)?;
            println!(
                "Removed workspace {} ({}) from {}",
                removed.name,
                removed.id,
                removed.config_path.display()
            );
            println!("Its files and MEDULLA.md are untouched.");
            Ok(())
        }
    }
}

/// `medulla workspace add [dir]` — write the profile, then register the
/// directory so the orchestrator can place work in it.
async fn add(
    parsed: &WorkspaceArgs,
    loaded: &LoadedConfig,
    config_path: &Path,
    dir: &Path,
) -> anyhow::Result<()> {
    let outcome = medulla::init::init_and_register_workspace(
        dir,
        config_path,
        &loaded.config,
        parsed.harness.as_deref(),
        parsed.force,
    )
    .await?;

    report_profile(&outcome);
    if let Some(reg) = &outcome.registration {
        report_registration(reg);
    }
    if let Some(err) = &outcome.registration_error {
        // Registering is the whole point of `add`, so a failure here fails the
        // command — exiting 0 would tell a script the workspace is enrolled when
        // it is not. The profile is still on disk, and saying so distinguishes
        // this from a run that did nothing at all.
        return Err(anyhow::anyhow!(
            "{err}\nThe profile at {} was written; only registration failed.",
            outcome.path.display()
        ));
    }
    Ok(())
}

/// Report what was written to `MEDULLA.md`. Shared with `medulla init`, which
/// does exactly this much and no registration.
pub(crate) fn report_profile(outcome: &InitOutcome) {
    if outcome.kept_profile {
        println!("Kept the existing {}.", outcome.path.display());
        println!("Pass --force to redraft it from the directory's instruction files.");
        return;
    }
    if outcome.drafted {
        println!(
            "Wrote {} (drafted from {})",
            outcome.path.display(),
            outcome.sources.join(", ")
        );
        println!("Review it — the summary is what the orchestrator reads.");
    } else {
        println!("Wrote {} (stub)", outcome.path.display());
        println!("Fill in the summary and routing hints, then it is ready to use.");
    }
    println!("Mapped {} paths into its layout.", outcome.layout.len());
}

/// Report a completed registration, naming the file it landed in so the operator
/// knows which config now owns the entry.
fn report_registration(reg: &WorkspaceRegistration) {
    println!(
        "{} workspace {} ({}) on harness {} in {}",
        if reg.added { "Registered" } else { "Refreshed" },
        reg.name,
        reg.id,
        reg.harness_id,
        reg.config_path.display(),
    );
    println!("The orchestrator can now place work in {}.", reg.path);
}

/// `medulla workspace list` — print the registry, as a table or as JSON.
///
/// Reads the *layered* config, so this shows what the orchestrator would
/// actually see, not just what the writable file happens to hold.
fn list(loaded: &LoadedConfig, config_path: &Path, json: bool) {
    let workspaces = &loaded.config.fleet.workspaces;
    if json {
        let rows: Vec<serde_json::Value> = workspaces
            .iter()
            .map(|w| {
                serde_json::json!({
                    "id": w.id,
                    "name": w.name,
                    "path": w.path,
                    "harnessId": w.harness_id,
                    "project": w.project,
                    "hasProfile": has_profile(&w.path),
                })
            })
            .collect();
        // Serialising a plain array of owned values cannot fail.
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
        );
        return;
    }

    if workspaces.is_empty() {
        println!("No workspaces registered in {}.", config_path.display());
        println!("Run `medulla workspace add [dir]` to register one.");
        return;
    }
    println!("{} registered workspace(s):", workspaces.len());
    for w in workspaces {
        // A registered workspace whose profile was never authored is the one
        // failure mode worth calling out inline: the orchestrator can place work
        // there but has nothing describing it.
        let profile = if has_profile(&w.path) {
            ""
        } else {
            "  [no MEDULLA.md — run `medulla init` there]"
        };
        println!("  {}  {}  ({}){}", w.id, w.path, w.harness_id, profile);
    }
}

/// Whether a registered path still has an authored profile on disk.
fn has_profile(path: &str) -> bool {
    medulla::init::read_medulla_md(Path::new(path)).is_some()
}

/// The config file registry writes land in.
///
/// Mirrors the TUI's own choice (see `app_loop`): an explicit `--config` wins,
/// then the highest-precedence file that actually contributed to the layered
/// load, then the home config when nothing was discovered. Writing to the file
/// the operator is already using is what keeps this command and the running TUI
/// looking at one registry.
fn registry_config_path(
    explicit: Option<&str>,
    loaded: &LoadedConfig,
    env: &HashMap<String, String>,
) -> PathBuf {
    explicit
        .map(PathBuf::from)
        .or_else(|| loaded.sources.last().map(PathBuf::from))
        .unwrap_or_else(|| medulla::home::medulla_home(env).join("config.toml"))
}
