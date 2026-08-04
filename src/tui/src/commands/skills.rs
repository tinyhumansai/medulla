//! `medulla skills` — put harness-native skills for saved workflows on disk.
//!
//! A saved workflow is invisible to the operator's everyday Claude Code or
//! Codex session: nothing there knows it exists or that `workflow_run` is how
//! to start it. This command closes that gap by writing a skill per workflow
//! into the harness's own layout, and — with `--with-mcp` — registering the
//! server whose tool the skill tells the model to call.
//!
//! Every decision that touches the filesystem lives in
//! [`medulla::workflows::skills`], so the future MCP verb and the TUI action
//! reuse it rather than re-deriving paths. What is here is the CLI's own job:
//! turning flags into an [`InstallOptions`], resolving the store, and printing.
//!
//! Output follows the `workflow` command's contract — results on stdout, errors
//! on stderr with a non-zero exit — with one addition: the default is a human
//! summary, and `--json` asks for the machine shape. The report is printed
//! *before* any refusal is raised, so the exit status a CI wrapper reads and the
//! detail a human reads never disagree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use medulla::workflows::skills::{
    self, InstallOptions, InstallReport, RegistrationOptions, RegistrationOutcome, SkillTarget,
};
use medulla::workflows::{ops, WorkflowSummary};
use medulla_tui::cli::{parse_skills_args, SkillsAction, SkillsArgs};
use serde_json::{json, Value};

/// Run `medulla skills <action>`.
///
/// # Errors
///
/// Returns an error when a named workflow does not exist, when no harness can
/// be written to, when `--with-mcp` cannot be honoured, or when the filesystem
/// refuses a write. It also fails *after* printing a full report in two cases
/// that are easy to miss in a scrollback: a bare `uninstall` with no `--all`,
/// and any run whose report contains a collision — a workflow the operator
/// asked for that is, in fact, not installed. A malformed flag exits 2 before
/// any of that.
pub(crate) fn run_skills_cmd(args: &[String]) -> anyhow::Result<()> {
    let parsed = match parse_skills_args(args) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("medulla skills: {msg}");
            std::process::exit(2);
        }
    };

    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = resolve_root(&parsed, &env, &cwd);
    let targets = match &parsed.targets {
        Some(chosen) => chosen.clone(),
        None => skills::default_targets(&root),
    };

    if targets.is_empty() {
        anyhow::bail!(
            "no harness is set up under {}: none of .claude, .codex, or .medulla exists there. \
             Name one with --harness claude|codex|generic (or --harness all), or point --dir \
             somewhere else.",
            root.display()
        );
    }

    let opts = InstallOptions {
        targets: targets.clone(),
        scope: parsed.scope,
        root: root.clone(),
        with_commands: parsed.with_commands,
        dry_run: parsed.dry_run,
    };

    // Checked before anything is written or registered: "remove everything" is
    // one typo away from "remove this one", and the removal is not recoverable
    // from the output.
    if parsed.action == SkillsAction::Uninstall && parsed.ids.is_empty() && !parsed.all {
        return refuse_blanket_uninstall(&parsed, &root, &targets, &opts);
    }

    // Asked for before anything is written: a registration that cannot serve
    // tools makes every skill we are about to install inert, and half-doing it
    // is worse than refusing.
    if parsed.with_mcp {
        medulla::mcp::preflight(&env, &cwd).map_err(|msg| anyhow::anyhow!("--with-mcp: {msg}"))?;
    }

    let output = match parsed.action {
        SkillsAction::List => list(&opts)?,
        SkillsAction::Install => {
            let store = ops::discover_store(&env, &cwd);
            let workflows = selected(&store, &parsed.ids)?;
            report_value(&skills::install(&workflows, &opts)?)
        }
        SkillsAction::Sync => {
            let store = ops::discover_store(&env, &cwd);
            // The named ids, exactly as `install` reads them. Syncing the whole
            // store when one workflow was named would rewrite skills the
            // operator did not ask about; `--prune` alongside ids is refused by
            // the parser, so the keep set here is never a partial one.
            let workflows = selected(&store, &parsed.ids)?;
            report_value(&skills::sync(&workflows, &opts, parsed.prune)?)
        }
        SkillsAction::Uninstall => {
            let ids = if parsed.ids.is_empty() {
                installed_ids(&opts)?
            } else {
                parsed.ids.clone()
            };
            report_value(&skills::uninstall(&ids, &opts)?)
        }
    };

    let registrations = if parsed.with_mcp {
        register(&parsed, &targets, &root, &project_dir(&parsed, &root, &cwd))?
    } else {
        Vec::new()
    };

    print(&parsed, &root, &targets, &output, &registrations);

    // Last, so the report is on stdout either way: a collision means a workflow
    // the operator asked for is *not* installed, and an exit status is the only
    // part of this a CI wrapper reads.
    if output.get("collisions").and_then(Value::as_bool) == Some(true) {
        anyhow::bail!(
            "not every skill was installed: a path already held a file Medulla did not write, or \
             two workflows wanted the same skill directory. Nothing was overwritten — the flagged \
             entries above name the files, and those workflows have no skill."
        );
    }
    Ok(())
}

/// Print what a bare `uninstall` would have deleted, then refuse it.
///
/// The listing is the point: an operator who meant `--all` learns exactly what
/// they are about to lose, and one who mistyped an id learns that they did.
fn refuse_blanket_uninstall(
    parsed: &SkillsArgs,
    root: &Path,
    targets: &[SkillTarget],
    opts: &InstallOptions,
) -> anyhow::Result<()> {
    let found = list(opts)?;
    print(parsed, root, targets, &found, &[]);
    let count = found
        .get("skills")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    anyhow::bail!(
        "refusing to remove all {count} managed skills listed above without being asked to: name \
         the workflow ids to remove (`medulla skills uninstall <id>…`), or pass --all to mean \
         every one of them."
    )
}

/// The root skills are written under.
///
/// `--dir` wins over `--scope`, and a relative `--dir` is resolved against the
/// working directory here rather than in the SDK, which is handed an already
/// absolute root on purpose.
fn resolve_root(parsed: &SkillsArgs, env: &HashMap<String, String>, cwd: &Path) -> PathBuf {
    match parsed.dir.as_deref() {
        Some(dir) => {
            let path = PathBuf::from(dir);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        }
        None => skills::scope_root(parsed.scope, env, cwd),
    }
}

/// The checkout that owns `.mcp.json`.
///
/// `--dir` overrides `--scope`, and it has to override this too: an operator who
/// says `--scope project --dir /srv/checkout` is telling us *that* directory is
/// the project. Reading the working directory instead would drop an unrequested
/// `.mcp.json` into whichever repository they happened to be standing in, which
/// is both a surprise and a file they may well commit. Without `--dir` the
/// working directory is still the answer: it is what `--scope project` resolved
/// the root from, and Claude's project config is a property of the checkout
/// rather than of the scope.
fn project_dir(parsed: &SkillsArgs, root: &Path, cwd: &Path) -> PathBuf {
    if parsed.dir.is_some() {
        root.to_path_buf()
    } else {
        cwd.to_path_buf()
    }
}

/// The workflows to install: the named ids, or every one in the store.
///
/// A named id that the store does not have is an error rather than a silent
/// omission — the operator asked for that workflow by name, and a success
/// message listing three of the four files they expected teaches nothing.
/// Disabled workflows are filtered by the installer itself, so they are passed
/// through here.
fn selected(
    store: &std::sync::Arc<dyn medulla::workflows::WorkflowStore>,
    ids: &[String],
) -> anyhow::Result<Vec<WorkflowSummary>> {
    let all = store.list()?;
    if ids.is_empty() {
        return Ok(all);
    }
    let mut chosen = Vec::with_capacity(ids.len());
    for id in ids {
        let found = all
            .iter()
            .find(|summary| &summary.id == id)
            .ok_or_else(|| anyhow::anyhow!("no workflow with id {id:?} is installed"))?;
        chosen.push(found.clone());
    }
    Ok(chosen)
}

/// Every workflow id that currently has a managed skill on disk.
///
/// What `uninstall --all` means: remove what is there. Deriving the set from
/// the store instead would leave a skill for a deleted workflow behind, which
/// is precisely the file an operator runs `uninstall` to be rid of. The same
/// listing is what a bare, id-less `uninstall` prints before refusing.
fn installed_ids(opts: &InstallOptions) -> anyhow::Result<Vec<String>> {
    let mut ids: Vec<String> = skills::installed(opts)?
        .into_iter()
        .map(|skill| skill.workflow_id)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The `list` payload: what is installed, and under which harness.
fn list(opts: &InstallOptions) -> anyhow::Result<Value> {
    let found = skills::installed(opts)?;
    Ok(json!({ "skills": found }))
}

/// An install/sync/uninstall report as JSON.
fn report_value(report: &InstallReport) -> Value {
    json!({
        "files": report.files,
        "collisions": report.has_collisions(),
    })
}

/// Register `medulla mcp` with each target.
///
/// `project_dir` is where `.mcp.json` goes — [`project_dir`] decides it, and it
/// is not always the working directory. The command is the *resolved* path of
/// this binary, never the bare name: a user-scope registration is read by a
/// process whose `PATH` we do not control.
fn register(
    parsed: &SkillsArgs,
    targets: &[SkillTarget],
    root: &Path,
    project_dir: &Path,
) -> anyhow::Result<Vec<RegistrationOutcome>> {
    let exe = std::env::current_exe()
        .map_err(|err| anyhow::anyhow!("--with-mcp: cannot resolve this binary's path: {err}"))?;
    let opts = RegistrationOptions {
        targets: targets.to_vec(),
        scope: parsed.scope,
        root: root.to_path_buf(),
        project_dir: project_dir.to_path_buf(),
        exe,
        tools_mode: parsed.tools.clone(),
        dry_run: parsed.dry_run,
    };
    Ok(skills::register(&opts)?)
}

/// Emit the result: one JSON document, or the human summary.
fn print(
    parsed: &SkillsArgs,
    root: &Path,
    targets: &[SkillTarget],
    output: &Value,
    registrations: &[RegistrationOutcome],
) {
    let names: Vec<&str> = targets.iter().map(|target| target.as_str()).collect();

    if parsed.json {
        let mut document = json!({
            "root": root,
            "scope": parsed.scope.as_str(),
            "targets": names,
            "dryRun": parsed.dry_run,
        });
        if let (Some(map), Some(extra)) = (document.as_object_mut(), output.as_object()) {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
            if !registrations.is_empty() {
                map.insert("registrations".to_string(), json!(registrations));
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&document).unwrap_or_default()
        );
        return;
    }

    let dry = if parsed.dry_run { " (dry run)" } else { "" };
    println!("{} under {}{dry}", names.join(", "), root.display());

    if let Some(found) = output.get("skills").and_then(Value::as_array) {
        if found.is_empty() {
            println!("  no managed skills installed");
        }
        for skill in found {
            println!(
                "  {} {} {}",
                skill.get("target").and_then(Value::as_str).unwrap_or("?"),
                skill
                    .get("workflowId")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                skill.get("path").and_then(Value::as_str).unwrap_or("?"),
            );
        }
    }

    if let Some(files) = output.get("files").and_then(Value::as_array) {
        if files.is_empty() {
            println!("  nothing to do");
        }
        for file in files {
            println!(
                "  {} {}",
                file.get("action").and_then(Value::as_str).unwrap_or("?"),
                file.get("path").and_then(Value::as_str).unwrap_or("?"),
            );
        }
        if output.get("collisions").and_then(Value::as_bool) == Some(true) {
            println!(
                "  note: the flagged entries above were left exactly as they were — a file \
                 Medulla did not write, or two workflows wanting one skill directory. Those \
                 workflows are NOT installed."
            );
        }
    }

    for outcome in registrations {
        match &outcome.manual_command {
            Some(command) => println!(
                "  mcp {} {}: run this yourself: {command}",
                outcome.target, outcome.action
            ),
            None => println!(
                "  mcp {} {} {}",
                outcome.target,
                outcome.action,
                outcome
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            ),
        }
    }
}
