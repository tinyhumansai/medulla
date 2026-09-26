//! Containment: every path a tool touches must resolve inside the turn's
//! checkout.
//!
//! # Why resolution happens before the check, not after
//!
//! A turn is given a checkout (`cwd`) and the tools below take relative paths.
//! Checking the *textual* path for `..` is not enough — a symlink inside the
//! checkout pointing at `/etc` is a perfectly ordinary-looking relative path
//! that lands outside it. So the path is canonicalized first and the containment
//! test is applied to the result, which is the only form that reflects where a
//! write would actually go.
//!
//! # A path that does not exist yet still has to be checked
//!
//! `write_file` creates files *and their parent directories*, so canonicalizing
//! the target — or even its immediate parent — would fail on exactly the calls
//! that matter most. Resolution walks up to the deepest ancestor that does
//! exist, canonicalizes that, and re-attaches the components below it. A write
//! to `a/b/c.txt` in an empty checkout therefore resolves against the checkout
//! root, which is the only ancestor there is.
//!
//! The re-attached components are checked for `..` separately, because they were
//! never canonicalized and so could still climb out of the ancestor that was.

use std::path::{Path, PathBuf};

/// Why a path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path resolved outside the turn's checkout.
    #[error("path escapes the workspace: {0}")]
    Escapes(String),
    /// The path could not be resolved at all.
    #[error("path cannot be resolved: {0}")]
    Unresolvable(String),
}

/// Resolve `candidate` against `root` and confirm it stays inside.
///
/// `candidate` may be relative (resolved against `root`) or absolute (still
/// required to sit inside `root`). Returns the canonical path so the caller
/// operates on the same location the check approved — re-deriving it would
/// reopen the gap the check closed.
///
/// # Errors
///
/// [`PathError::Escapes`] when the resolved path is outside `root`, and
/// [`PathError::Unresolvable`] when neither the path nor its parent exists.
pub fn contained(root: &Path, candidate: &str) -> Result<PathBuf, PathError> {
    let root = root
        .canonicalize()
        .map_err(|e| PathError::Unresolvable(format!("workspace root: {e}")))?;
    let joined = {
        let raw = Path::new(candidate);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            root.join(raw)
        }
    };

    let resolved =
        resolve_deepest(&joined).ok_or_else(|| PathError::Unresolvable(candidate.to_string()))?;

    if !resolved.starts_with(&root) {
        return Err(PathError::Escapes(candidate.to_string()));
    }
    Ok(resolved)
}

/// Canonicalize the deepest existing ancestor of `path` and re-attach the rest.
///
/// `None` when no ancestor resolves at all, which on a rooted path means the
/// filesystem is gone from under us.
///
/// A `..` among the re-attached components is refused by returning the climb
/// unresolved rather than by normalising it: those components were never
/// canonicalized, so treating `existing/../../etc` as valid would let a write
/// leave the ancestor whose containment was actually verified.
///
/// # Dangling symlinks are refused for the same reason, and it is subtler
///
/// `canonicalize` fails on a symlink whose target does not exist, so a dangling
/// link inside the workspace pointing at `/etc/something` looks exactly like a
/// path that has not been created yet. Walking past it and re-attaching its
/// *name* under the canonical root produces a contained-looking path — and the
/// open that follows resolves the link and writes outside. The symlink test
/// beside this one only covered links whose targets exist, which is why the
/// containment check passed while the write escaped.
///
/// So every component re-attached below the resolved ancestor is checked with
/// `symlink_metadata`, which reports a dangling link where `canonicalize` and
/// `exists` both say nothing is there.
fn resolve_deepest(path: &Path) -> Option<PathBuf> {
    let mut trailing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(base) = cursor.canonicalize() {
            // Every component below the resolved ancestor is unverified. A
            // `..` here would climb back out of it.
            if trailing.iter().any(|c| *c == std::ffi::OsStr::new("..")) {
                return None;
            }
            let mut out = base;
            for component in trailing.into_iter().rev() {
                out.push(component);
                // Exists-as-a-link but does not resolve: a dangling symlink,
                // which an open would follow straight out of the workspace.
                if std::fs::symlink_metadata(&out).is_ok() {
                    return None;
                }
            }
            return Some(out);
        }
        let name = cursor.file_name()?;
        trailing.push(name);
        cursor = cursor.parent()?;
    }
}

#[cfg(test)]
mod tests;
