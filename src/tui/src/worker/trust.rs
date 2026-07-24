//! Clearing Claude Code's startup dialogs before a peer can meet them.
//!
//! Two modals stand between a fresh worker and a running task, and both are
//! answered here rather than at the first task:
//!
//! 1. **Workspace trust** — gated per directory, in the config file.
//! 2. **The bypass-permissions disclaimer** — raised by
//!    `--dangerously-skip-permissions`, in the settings file. Its default
//!    option is *"No, exit"*, so of the two this is the one where a blind
//!    Return does not mistype a prompt but kills the session.
//!
//! Claude gates a directory behind a modal workspace-trust dialog, and only in
//! interactive mode — it documents the dialog as skipped when stdout is not a
//! TTY. The headless daemon therefore never meets it; the worker TUI, whose
//! whole purpose is to give the harness a real TTY, always does. Left alone,
//! the first peer task on a fresh workspace dies waiting for a modal no one is
//! watching.
//!
//! Claude names the way out in its own error text: *"Run Claude Code
//! interactively here once and accept the trust dialog, or set
//! `projects[<path>].hasTrustDialogAccepted: true`"*. This module does the
//! latter, at startup, so an unattended worker is actually unattended.
//!
//! Treating that as the operator's decision rather than ours would be a fiction:
//! launching `medulla daemon --workspace X` *is* the statement that peer agent
//! work runs in X. So it is done automatically and said out loud in the log,
//! and `--no-trust-workspace` declines it.
//!
//! Care is taken because these are somebody else's files, ones Claude writes
//! too:
//!
//! - the file is parsed as free-form JSON and written back whole, so keys this
//!   code has never heard of survive;
//! - exactly one key is set, and only when no ancestor already grants trust;
//! - the write is atomic (temp file plus rename), because truncating a user's
//!   Claude config would be a far worse bug than the one being fixed;
//! - an absent or unparseable config is left alone — Claude will onboard the
//!   operator itself, and guessing at a config it has not written yet is not
//!   this module's business.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// What ensuring trust did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustOutcome {
    /// The workspace (or an ancestor) was already trusted; nothing was written.
    AlreadyTrusted,
    /// Trust was granted, in the config file at this path.
    Granted(PathBuf),
    /// Nothing was done, for this reason.
    Skipped(String),
}

impl TrustOutcome {
    /// A line for the operator's log, or `None` when there is nothing worth
    /// saying — the already-settled case is the normal one and narrating it
    /// every launch would be noise.
    ///
    /// `did` names the change in the past tense ("trusted /srv/work"), so the
    /// same outcome type can report either preflight step.
    pub fn log_line(&self, did: &str) -> Option<String> {
        match self {
            TrustOutcome::AlreadyTrusted => None,
            TrustOutcome::Granted(path) => Some(format!("claude: {did} (in {})", path.display())),
            TrustOutcome::Skipped(why) => Some(format!("claude: could not {did} — {why}")),
        }
    }
}

/// Where Claude keeps its user settings, honouring `CLAUDE_CONFIG_DIR`.
///
/// A different file from [`config_path`]: the trust flag lives in the config,
/// the bypass-disclaimer flag in the settings. Claude migrates the latter out
/// of the config into the settings, so the config key alone no longer works —
/// verified against the installed CLI, which kept showing the disclaimer until
/// the settings key was written.
pub fn settings_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    let dir = env
        .get("CLAUDE_CONFIG_DIR")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty());
    if let Some(dir) = dir {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    env.get("HOME")
        .map(|home| PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Accept Claude's bypass-permissions disclaimer ahead of time.
///
/// `--dangerously-skip-permissions` opens a second modal — "you accept all
/// responsibility" — whose **default option is `1. No, exit`**. That makes it
/// strictly more dangerous than the trust dialog for an unattended worker: a
/// stray Return does not mistype a prompt, it kills the session. So a worker
/// asked to run in bypass mode accepts the disclaimer up front rather than
/// meeting it, which is the same decision the operator already made by asking
/// for the mode.
///
/// Unlike the config, the settings file *is* created when absent: it is an
/// ordinary settings file holding no identity or onboarding state, and most
/// installs simply do not have one.
pub fn ensure_bypass_accepted(env: &HashMap<String, String>) -> TrustOutcome {
    let Some(path) = settings_path(env) else {
        return TrustOutcome::Skipped("no HOME or CLAUDE_CONFIG_DIR".to_string());
    };
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => value,
            Err(_) => {
                return TrustOutcome::Skipped(format!("{} is not valid JSON", path.display()))
            }
        },
        Err(_) => json!({}),
    };
    if settings.get(BYPASS_KEY).and_then(Value::as_bool) == Some(true) {
        return TrustOutcome::AlreadyTrusted;
    }
    if !settings.is_object() {
        settings = json!({});
    }
    settings
        .as_object_mut()
        .expect("just ensured object")
        .insert(BYPASS_KEY.to_string(), Value::Bool(true));

    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return TrustOutcome::Skipped(format!("could not create {}", parent.display()));
        }
    }
    match write_atomic(&path, &settings) {
        Ok(()) => TrustOutcome::Granted(path),
        Err(err) => TrustOutcome::Skipped(format!("could not write {}: {err}", path.display())),
    }
}

/// The settings key that suppresses the bypass-permissions disclaimer.
const BYPASS_KEY: &str = "skipDangerousModePermissionPrompt";

/// Where Claude keeps its config, honouring `CLAUDE_CONFIG_DIR`.
pub fn config_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    let dir = env
        .get("CLAUDE_CONFIG_DIR")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty());
    if let Some(dir) = dir {
        return Some(PathBuf::from(dir).join(".claude.json"));
    }
    env.get("HOME")
        .map(|home| PathBuf::from(home).join(".claude.json"))
}

/// Whether `dir` or any ancestor of it is already trusted.
///
/// The ancestor walk mirrors Claude's own: trusting a parent covers everything
/// beneath it, so a workspace inside an already-trusted tree needs no entry of
/// its own.
pub fn is_trusted(config: &Value, dir: &Path) -> bool {
    let Some(projects) = config.get("projects") else {
        return false;
    };
    dir.ancestors().any(|ancestor| {
        projects
            .get(ancestor.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("hasTrustDialogAccepted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

/// Set the trust flag for `dir`, preserving any entry already there.
pub fn grant(config: &mut Value, dir: &Path) {
    let key = dir.to_string_lossy().into_owned();
    if !config.is_object() {
        *config = json!({});
    }
    let projects = config
        .as_object_mut()
        .expect("just ensured object")
        .entry("projects")
        .or_insert_with(|| json!({}));
    if !projects.is_object() {
        *projects = json!({});
    }
    let entry = projects
        .as_object_mut()
        .expect("just ensured object")
        .entry(key)
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry
        .as_object_mut()
        .expect("just ensured object")
        .insert("hasTrustDialogAccepted".to_string(), Value::Bool(true));
}

/// Make sure `workspace` is trusted by Claude, writing the config if not.
///
/// Best-effort by design: every failure path is a `Skipped`, never an error
/// that stops the worker starting. The cost of skipping is one clear message on
/// the first task; the cost of refusing to start would be a worker that will
/// not run at all.
pub fn ensure_workspace_trusted(env: &HashMap<String, String>, workspace: &str) -> TrustOutcome {
    let Some(path) = config_path(env) else {
        return TrustOutcome::Skipped("no HOME or CLAUDE_CONFIG_DIR".to_string());
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return TrustOutcome::Skipped(format!("{} is not readable", path.display()));
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&text) else {
        return TrustOutcome::Skipped(format!("{} is not valid JSON", path.display()));
    };

    // Claude keys projects by resolved path, so a symlinked workspace must be
    // canonicalized or the entry would be written where nothing looks for it.
    let dir = std::fs::canonicalize(workspace).unwrap_or_else(|_| PathBuf::from(workspace));
    if is_trusted(&config, &dir) {
        return TrustOutcome::AlreadyTrusted;
    }
    grant(&mut config, &dir);

    match write_atomic(&path, &config) {
        Ok(()) => TrustOutcome::Granted(path),
        Err(err) => TrustOutcome::Skipped(format!("could not write {}: {err}", path.display())),
    }
}

/// Write `config` to `path` via a temp file and a rename.
///
/// Never truncates the original: a crash mid-write leaves the old config
/// intact, which matters because this file is the operator's, not ours.
fn write_atomic(path: &Path, config: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let temp = path.with_extension("json.medulla-tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}
