//! Workspace directory context for the capability probe.
//!
//! An orchestrator routing work across a roster needs to know *what project*
//! lives in each agent's working directory, not just which tools the agent has.
//! This module reads the well-known instruction files (CLAUDE.md, AGENTS.md,
//! README.md) from a workspace so the probe can ground its self-report prompt in
//! real file content, and derives a deterministic digest used as the summary when
//! the LLM probe fails. Reading never fails: missing or unreadable files are
//! skipped.

use std::path::{Path, PathBuf};

/// The well-known project files summarized into the roster, probe order.
pub const DIR_CONTEXT_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "README.md"];

/// Per-file cap on the excerpt embedded in the probe prompt.
pub const MAX_FILE_EXCERPT_CHARS: usize = 4_000;

/// Hard cap on any summary text (≈100 tokens at ~6 chars/token of prose).
pub const MAX_SUMMARY_CHARS: usize = 600;

/// Read CLAUDE.md/AGENTS.md/README.md under `cwd`. A symlinked pair (the common
/// `CLAUDE.md → AGENTS.md` convention) is included once, under the first name.
pub async fn read_dir_context(cwd: &str) -> DirContext {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    let mut digests: Vec<String> = Vec::new();

    for name in DIR_CONTEXT_FILES {
        let path = Path::new(cwd).join(name);
        // Dedupe by canonical path so a symlinked pair isn't included twice.
        let canonical = match tokio::fs::canonicalize(&path).await {
            Ok(canonical) => canonical,
            Err(_) => continue,
        };
        if seen.contains(&canonical) {
            continue;
        }
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        seen.push(canonical);
        sections.push(format!(
            "--- {name} (excerpt) ---\n{}",
            truncate_chars(trimmed, MAX_FILE_EXCERPT_CHARS)
        ));
        if let Some(digest) = digest_markdown(trimmed) {
            digests.push(format!("{name}: {digest}"));
        }
    }

    DirContext {
        prompt_block: (!sections.is_empty()).then(|| {
            format!(
                "Project files found in your working directory:\n{}",
                sections.join("\n")
            )
        }),
        fallback_summary: (!digests.is_empty())
            .then(|| truncate_chars(&digests.join(" · "), MAX_SUMMARY_CHARS)),
    }
}

/// First heading plus the first prose paragraph, whitespace-collapsed. Skips
/// YAML frontmatter, code fences, and image/badge-only lines.
fn digest_markdown(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    // Frontmatter: a leading `---` fence closed by the next `---`/`...` line.
    if lines.peek().map(|l| l.trim()) == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            let trimmed = line.trim();
            if trimmed == "---" || trimmed == "..." {
                break;
            }
        }
    }

    let mut heading: Option<String> = None;
    let mut paragraph: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break; // paragraph complete
        }
        if trimmed.starts_with('#') {
            if heading.is_none() && paragraph.is_empty() {
                heading = Some(trimmed.trim_start_matches('#').trim().to_string());
            } else {
                break; // next section starts; the first paragraph is done
            }
            continue;
        }
        // Skip image/badge-only lines (e.g. `[![CI](...)](...)` or `![logo](...)`).
        if trimmed.starts_with("![") || trimmed.starts_with("[![") {
            continue;
        }
        paragraph.push(trimmed.to_string());
    }

    let mut parts: Vec<String> = Vec::new();
    if let Some(heading) = heading {
        if !heading.is_empty() {
            parts.push(heading);
        }
    }
    if !paragraph.is_empty() {
        parts.push(paragraph.join(" "));
    }
    let joined = parts.join(" — ");
    let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// Cap `text` at `max_chars` characters, appending `…` when cut. Safe on any
/// UTF-8 (counts chars, not bytes).
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests;

mod types;
pub use types::DirContext;
