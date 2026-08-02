//! Scanning a workspace's file layout for the orchestrator.
//!
//! A `MEDULLA.md` summary says what a repository *is*; the layout says what is
//! actually in it. The orchestrator decomposes work against paths — "the client
//! lives in `src/sdk/`" is the difference between a routable instruction and a
//! guess — so `init` records a bounded map of the tree beside the prose.
//!
//! The scan is deliberately shallow and cheap: top-level entries plus one level
//! inside each directory, ignoring build output and vendored trees. It runs on
//! every `init`, including the offline path, so it must never be slow enough to
//! notice or large enough to crowd an orchestrator's context.

use std::fs;
use std::path::Path;

/// How many entries the whole map may list. A repository with hundreds of
/// top-level files is not made more routable by naming all of them, and the map
/// rides in every session's context.
const MAX_ENTRIES: usize = 40;

/// How many children are named inside one directory before it is summarised as
/// a count.
const MAX_CHILDREN: usize = 8;

/// Directories never worth describing: build output, dependency caches, and
/// version-control internals. Matched on the exact directory name at any depth.
const IGNORED: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".next",
    ".turbo",
    ".cargo",
    ".medulla-state",
    "vendor",
];

/// Whether a directory entry is scanned at all. Hidden entries are skipped —
/// dotfiles are configuration, not the shape of the codebase — except that a
/// hidden *file* at the top level is still uninteresting, so the same rule
/// covers both.
fn is_ignored(name: &str) -> bool {
    name.starts_with('.') || IGNORED.contains(&name)
}

/// Read one directory's entries as `(name, is_dir)`, sorted directories-first
/// then alphabetically, with ignored names removed.
///
/// An unreadable directory yields no entries rather than an error: `init` is
/// best-effort over whatever the operator can actually see.
fn entries(dir: &Path) -> Vec<(String, bool)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<(String, bool)> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_ignored(&name) {
                return None;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows
}

/// Scan `dir` into a bounded list of layout lines, each a path relative to the
/// workspace root (directories keep their trailing `/`).
///
/// Two levels deep, at most `MAX_ENTRIES` lines. A directory with more than
/// `MAX_CHILDREN` children contributes its named children plus a `… N more`
/// line, so the orchestrator can tell "small module" from "large one" without
/// being handed the whole tree. Returns an empty vector for an unreadable or
/// empty directory.
pub fn scan_layout(dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for (name, is_dir) in entries(dir) {
        if lines.len() >= MAX_ENTRIES {
            break;
        }
        if !is_dir {
            lines.push(name);
            continue;
        }
        lines.push(format!("{name}/"));

        let children = entries(&dir.join(&name));
        let shown = children.len().min(MAX_CHILDREN);
        for (child, child_is_dir) in children.iter().take(shown) {
            if lines.len() >= MAX_ENTRIES {
                break;
            }
            let suffix = if *child_is_dir { "/" } else { "" };
            lines.push(format!("{name}/{child}{suffix}"));
        }
        if children.len() > shown && lines.len() < MAX_ENTRIES {
            lines.push(format!("{name}/… {} more", children.len() - shown));
        }
    }
    lines
}

/// Render a scanned layout as the `MEDULLA.md` layout block: one indented line
/// per entry, inside the `layout: |` block scalar.
///
/// An empty scan renders a single placeholder line so the block stays
/// well-formed rather than emitting a dangling `layout: |`.
pub fn layout_block(lines: &[String]) -> String {
    if lines.is_empty() {
        return "  (empty workspace)".to_string();
    }
    lines
        .iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
