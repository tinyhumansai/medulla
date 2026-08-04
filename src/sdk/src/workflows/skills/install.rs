//! Writing, refreshing, and removing generated skills — the managed-file
//! discipline that makes it safe to touch `~/.claude`.
//!
//! One rule governs every path in here: a file we did not write is never
//! overwritten and never deleted. Ownership is proved by the marker line
//! [`super::render`] puts at the top of everything it generates, so a
//! hand-written `~/.claude/skills/medulla-babysit/SKILL.md` survives an install
//! intact and is reported as a collision instead.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::workflows::WorkflowSummary;

use super::render::{parse_marker, render, render_command};
use super::targets::{command_path, commands_dir, skill_path, skills_dir};
use super::{FileAction, FileOutcome, InstallOptions, InstallReport, InstalledSkill, SkillTarget};

/// Installs the given workflows into every configured target.
///
/// Disabled workflows are skipped entirely — a workflow that may not run should
/// not be advertised to a harness as runnable. Command files are written only
/// when [`InstallOptions::with_commands`] is set and the target has a command
/// layout.
///
/// # Errors
///
/// Propagates filesystem errors from creating directories or writing files.
/// A collision is *not* an error: it lands in the report as
/// [`FileAction::SkippedUnmanaged`] or [`FileAction::SlugCollision`] so one bad
/// path does not abort the rest.
pub fn install(workflows: &[WorkflowSummary], opts: &InstallOptions) -> io::Result<InstallReport> {
    let mut report = InstallReport::default();
    // Which workflow has already claimed each path in *this* run. Without it a
    // dry run would report two `Created`s where the real run writes one file
    // and collides on the second, and the two must agree.
    let mut claimed: HashMap<PathBuf, String> = HashMap::new();
    for target in &opts.targets {
        for summary in workflows.iter().filter(|summary| summary.enabled) {
            let skill = render(summary);
            let path = skill_path(*target, &opts.root, &skill.slug);
            report.files.push(write_managed(
                &path,
                &skill.body,
                *target,
                &summary.id,
                opts,
                &mut claimed,
            )?);

            if opts.with_commands {
                if let Some(path) = command_path(*target, &opts.root, &skill.slug) {
                    let body = render_command(&skill, summary);
                    report.files.push(write_managed(
                        &path,
                        &body,
                        *target,
                        &summary.id,
                        opts,
                        &mut claimed,
                    )?);
                }
            }
        }
    }
    Ok(report)
}

/// Installs `workflows` and, with `prune`, removes managed files for anything
/// else.
///
/// The pruned set is "every managed file under the target directories whose
/// workflow is not in `workflows`" — which covers a deleted workflow, a renamed
/// one, and a workflow that has since been disabled (disabled workflows are not
/// installed, so they are not in the keep set either). Unmarked neighbours are
/// left alone, as everywhere else.
///
/// # Errors
///
/// Propagates filesystem errors from the install pass or from removal.
pub fn sync(
    workflows: &[WorkflowSummary],
    opts: &InstallOptions,
    prune: bool,
) -> io::Result<InstallReport> {
    let mut report = install(workflows, opts)?;
    if !prune {
        return Ok(report);
    }

    let keep: BTreeSet<&str> = workflows
        .iter()
        .filter(|summary| summary.enabled)
        .map(|summary| summary.id.as_str())
        .collect();

    for target in &opts.targets {
        for managed in scan_managed(*target, &opts.root)? {
            if keep.contains(managed.workflow_id.as_str()) {
                continue;
            }
            report.files.push(remove_managed(&managed, opts)?);
        }
    }
    Ok(report)
}

/// Removes the managed skills (and their commands) for the named workflow ids.
///
/// Ids that are not installed are simply absent from the report; asking to
/// uninstall something twice is not an error.
///
/// # Errors
///
/// Propagates filesystem errors from reading the target directories or
/// removing files.
pub fn uninstall(ids: &[String], opts: &InstallOptions) -> io::Result<InstallReport> {
    let wanted: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let mut report = InstallReport::default();
    for target in &opts.targets {
        for managed in scan_managed(*target, &opts.root)? {
            if !wanted.contains(managed.workflow_id.as_str()) {
                continue;
            }
            report.files.push(remove_managed(&managed, opts)?);
        }
    }
    Ok(report)
}

/// Every managed skill currently on disk under the configured targets.
///
/// Reads markers rather than filenames, so a skill installed by an older
/// release under a different slug is still recognised as ours.
///
/// # Errors
///
/// Propagates filesystem errors other than a missing target directory, which
/// simply contributes nothing.
pub fn installed(opts: &InstallOptions) -> io::Result<Vec<InstalledSkill>> {
    let mut found = Vec::new();
    for target in &opts.targets {
        for managed in scan_managed(*target, &opts.root)? {
            if !managed.is_skill {
                continue;
            }
            found.push(InstalledSkill {
                workflow_id: managed.workflow_id,
                slug: managed.slug,
                target: *target,
                path: managed.path,
                rev: managed.rev,
            });
        }
    }
    Ok(found)
}

/// A generated file discovered on disk, with what its marker claims.
struct ManagedFile {
    path: PathBuf,
    slug: String,
    workflow_id: String,
    rev: String,
    target: SkillTarget,
    /// `true` for a `SKILL.md`, `false` for a slash-command file.
    is_skill: bool,
}

/// Writes `body` to `path` unless something else is already there.
///
/// The five outcomes are the whole contract: absent → written, ours and
/// identical → untouched, ours and stale → rewritten, someone else's → left
/// alone as [`FileAction::SkippedUnmanaged`], another workflow's → left alone
/// as [`FileAction::SlugCollision`]. `claimed` carries the paths earlier
/// workflows in this same run took, so the second of two workflows that
/// slugify alike collides instead of overwriting.
///
/// Under `dry_run` the identical decision is made and reported, and nothing is
/// written — including the parent directories, so a dry run leaves no trace.
fn write_managed(
    path: &Path,
    body: &str,
    target: SkillTarget,
    workflow_id: &str,
    opts: &InstallOptions,
    claimed: &mut HashMap<PathBuf, String>,
) -> io::Result<FileOutcome> {
    let outcome = |action| FileOutcome {
        path: path.to_path_buf(),
        target,
        workflow_id: workflow_id.to_string(),
        action,
    };

    if claimed.get(path).is_some_and(|owner| owner != workflow_id) {
        return Ok(outcome(FileAction::SlugCollision));
    }

    let action = match read_existing(path)? {
        Existing::Absent => FileAction::Created,
        // Bytes we cannot even read as text are certainly not a marker of ours.
        Existing::Foreign => FileAction::SkippedUnmanaged,
        Existing::Text(text) => match parse_marker(&text) {
            None => FileAction::SkippedUnmanaged,
            // Ours, but generated for another workflow: the path is spoken for
            // and saying "unmanaged" would blame a third party for our file.
            Some((marked_id, _)) if marked_id != workflow_id => FileAction::SlugCollision,
            Some((_, rev)) if marker_rev_of(body).as_deref() == Some(rev.as_str()) => {
                FileAction::Unchanged
            }
            Some(_) => FileAction::Updated,
        },
    };

    if matches!(
        action,
        FileAction::Created | FileAction::Updated | FileAction::Unchanged
    ) {
        claimed.insert(path.to_path_buf(), workflow_id.to_string());
    }

    if matches!(action, FileAction::Created | FileAction::Updated) && !opts.dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, body)?;
    }
    Ok(outcome(action))
}

/// The `rev` of freshly rendered content, read back off its own marker.
fn marker_rev_of(body: &str) -> Option<String> {
    parse_marker(body).map(|(_, rev)| rev)
}

/// Deletes one managed file, and its now-empty skill directory.
///
/// Leaving `medulla-babysit/` behind as an empty directory would make an
/// uninstalled skill look installed to anyone reading the tree.
fn remove_managed(managed: &ManagedFile, opts: &InstallOptions) -> io::Result<FileOutcome> {
    if !opts.dry_run {
        match fs::remove_file(&managed.path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        if managed.is_skill {
            if let Some(parent) = managed.path.parent() {
                // Best effort: a directory the operator put other files in
                // stays, and that is the desired outcome.
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(FileOutcome {
        path: managed.path.clone(),
        target: managed.target,
        workflow_id: managed.workflow_id.clone(),
        action: FileAction::Removed,
    })
}

/// Every marked file under one target's skill and command directories.
///
/// Missing directories yield nothing rather than an error: not having a
/// `.codex` is the normal state for most machines.
fn scan_managed(target: SkillTarget, root: &Path) -> io::Result<Vec<ManagedFile>> {
    let mut found = Vec::new();

    for entry in read_dir_opt(&skills_dir(target, root))? {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let path = dir.join("SKILL.md");
        let slug = entry.file_name().to_string_lossy().into_owned();
        if let Some((workflow_id, rev)) = read_marker(&path)? {
            found.push(ManagedFile {
                path,
                slug,
                workflow_id,
                rev,
                target,
                is_skill: true,
            });
        }
    }

    if let Some(dir) = commands_dir(target, root) {
        for entry in read_dir_opt(&dir)? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let slug = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some((workflow_id, rev)) = read_marker(&path)? {
                found.push(ManagedFile {
                    path,
                    slug,
                    workflow_id,
                    rev,
                    target,
                    is_skill: false,
                });
            }
        }
    }

    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// What is already at a path we are about to consider.
enum Existing {
    /// Nothing is there.
    Absent,
    /// Something is there that is not UTF-8 text, so it cannot be ours.
    Foreign,
    /// Readable text, marker or not.
    Text(String),
}

/// Reads a candidate file, treating undecodable bytes as someone else's.
///
/// Reading bytes rather than text is load-bearing: a single non-UTF-8 file
/// anywhere in `~/.claude/commands` would otherwise fail the whole scan with an
/// `InvalidData` error and abort a sync, uninstall, or listing that has nothing
/// to do with that file.
fn read_existing(path: &Path) -> io::Result<Existing> {
    match fs::read(path) {
        Ok(bytes) => Ok(match String::from_utf8(bytes) {
            Ok(text) => Existing::Text(text),
            Err(_) => Existing::Foreign,
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Existing::Absent),
        Err(err) => Err(err),
    }
}

/// The marker of the file at `path`, or `None` when it is absent or not ours.
fn read_marker(path: &Path) -> io::Result<Option<(String, String)>> {
    Ok(match read_existing(path)? {
        Existing::Text(text) => parse_marker(&text),
        Existing::Absent | Existing::Foreign => None,
    })
}

/// Directory entries, treating a missing directory as empty.
fn read_dir_opt(dir: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    entries.collect::<io::Result<Vec<_>>>()
}
