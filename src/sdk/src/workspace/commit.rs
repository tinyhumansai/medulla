//! Guarded exact-path Git commits.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use globset::Glob;
use regex::Regex;

use super::types::{CommitError, CommitOptions, CommitOutcome};

const STATUS_ARGS: [&str; 3] = ["status", "--porcelain=v1", "-z"];
const STAGED_PATH_ARGS: [&str; 4] = ["diff", "--cached", "--name-only", "-z"];
const SHORT_ID_ARGS: [&str; 3] = ["rev-parse", "--short", "HEAD"];

fn run_git<I, S>(workspace: &Path, operation: &'static str, args: I) -> Result<Output, CommitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|source| CommitError::Spawn { operation, source })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(CommitError::Git {
            operation,
            workspace: workspace.to_path_buf(),
            message: String::from_utf8_lossy(&output.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        })
    }
}

fn nul_paths(output: &[u8]) -> BTreeSet<PathBuf> {
    output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| PathBuf::from(String::from_utf8_lossy(field).into_owned()))
        .collect()
}

fn validate_path(path: &Path) -> Result<(), CommitError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CommitError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn validate_subject(subject: &str) -> Result<(), CommitError> {
    let conventional = Regex::new(
        r"^(feat|fix|refactor|chore|docs|test|build|ci|perf|style|revert)(\([a-zA-Z0-9._/-]+\))?!?: [^\s].*$",
    )
    .expect("static conventional-commit regex");
    if subject.contains('\n') || !conventional.is_match(subject) {
        return Err(CommitError::InvalidSubject);
    }
    Ok(())
}

fn validate_shared_paths(
    paths: &BTreeSet<PathBuf>,
    options: &CommitOptions,
) -> Result<(), CommitError> {
    if options.allow_shared || options.shared_path_denylist.is_empty() {
        return Ok(());
    }

    for pattern in &options.shared_path_denylist {
        let glob = Glob::new(pattern).map_err(|error| CommitError::InvalidPattern {
            pattern: pattern.clone(),
            message: error.to_string(),
        })?;
        let matcher = glob.compile_matcher();
        if let Some(path) = paths.iter().find(|path| matcher.is_match(path)) {
            return Err(CommitError::SharedPath {
                path: path.clone(),
                pattern: pattern.clone(),
            });
        }
    }
    Ok(())
}

/// Stage and commit exactly the named repository-relative paths.
///
/// The operation refuses clean paths, foreign staged changes, malformed
/// conventional subjects, unsafe paths, and guarded shared paths. A snapshot of
/// the index is restored if staging or committing fails.
pub fn commit(
    workspace: &Path,
    paths: &[PathBuf],
    subject: &str,
    options: &CommitOptions,
) -> Result<CommitOutcome, CommitError> {
    validate_subject(subject)?;
    if paths.is_empty() {
        return Err(CommitError::EmptyPaths);
    }

    let mut named = BTreeSet::new();
    for path in paths {
        validate_path(path)?;
        named.insert(path.clone());
    }
    validate_shared_paths(&named, options)?;

    run_git(
        workspace,
        "repository discovery",
        ["rev-parse", "--show-toplevel"],
    )?;
    let mut status_args = STATUS_ARGS.to_vec();
    status_args.push("--untracked-files=all");
    let status = run_git(workspace, "status", status_args)?;
    let changed = parse_changed_paths(&status.stdout);
    for path in &named {
        if !changed.contains(path) {
            return Err(CommitError::Unmodified(path.clone()));
        }
    }

    let staged = run_git(workspace, "staged-path inspection", STAGED_PATH_ARGS)?;
    for path in nul_paths(&staged.stdout) {
        if !named.contains(&path) {
            return Err(CommitError::ForeignStaged(path));
        }
    }

    let index_tree = run_git(workspace, "index snapshot", ["write-tree"])?;
    let index_tree = String::from_utf8_lossy(&index_tree.stdout)
        .trim()
        .to_owned();
    let mut add_args = vec!["add".into(), "--".into()];
    add_args.extend(named.iter().map(|path| path.as_os_str().to_owned()));
    if let Err(error) = run_git(workspace, "staging", add_args) {
        let _ = run_git(workspace, "index restore", ["read-tree", &index_tree]);
        return Err(error);
    }

    let mut commit_args = vec![
        OsStr::new("commit").to_owned(),
        OsStr::new("-m").to_owned(),
        OsStr::new(subject).to_owned(),
    ];
    if let Some(body) = options
        .body
        .as_deref()
        .filter(|body| !body.trim().is_empty())
    {
        commit_args.push(OsStr::new("-m").to_owned());
        commit_args.push(OsStr::new(body).to_owned());
    }
    commit_args.push(OsStr::new("--").to_owned());
    commit_args.extend(named.iter().map(|path| path.as_os_str().to_owned()));
    if let Err(error) = run_git(workspace, "commit", commit_args) {
        let _ = run_git(workspace, "index restore", ["read-tree", &index_tree]);
        return Err(error);
    }

    let id = run_git(workspace, "commit id", ["rev-parse", "HEAD"])?;
    let id = String::from_utf8_lossy(&id.stdout).trim().to_owned();
    let short_id = run_git(workspace, "short commit id", SHORT_ID_ARGS)?;
    Ok(CommitOutcome {
        short_id: String::from_utf8_lossy(&short_id.stdout).trim().to_owned(),
        id,
        subject: subject.to_owned(),
        paths: named.into_iter().collect(),
    })
}

pub(super) fn parse_changed_paths(output: &[u8]) -> BTreeSet<PathBuf> {
    let mut fields = output.split(|byte| *byte == 0);
    let mut paths = BTreeSet::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }
        let renamed = matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C');
        paths.insert(PathBuf::from(
            String::from_utf8_lossy(&field[3..]).into_owned(),
        ));
        if renamed {
            if let Some(original) = fields.next().filter(|value| !value.is_empty()) {
                paths.insert(PathBuf::from(
                    String::from_utf8_lossy(original).into_owned(),
                ));
            }
        }
    }
    paths
}
