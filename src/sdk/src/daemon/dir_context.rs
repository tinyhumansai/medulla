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

/// What the workspace's project files yield for one probe.
#[derive(Debug, Clone, Default)]
pub struct DirContext {
    /// Labeled, trimmed excerpts to append to the probe prompt; `None` when the
    /// workspace has none of the well-known files.
    pub prompt_block: Option<String>,
    /// Deterministic ≤[`MAX_SUMMARY_CHARS`] digest, the summary of last resort
    /// when the LLM probe fails or omits one.
    pub fallback_summary: Option<String>,
}

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
mod tests {
    use super::*;

    async fn write(dir: &Path, name: &str, content: &str) {
        tokio::fs::write(dir.join(name), content).await.unwrap();
    }

    #[tokio::test]
    async fn reads_all_three_files_into_prompt_and_digest() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "CLAUDE.md", "# Guide\n\nUse pnpm test.").await;
        write(dir.path(), "AGENTS.md", "# Agents\n\nRun cargo test.").await;
        write(dir.path(), "README.md", "# Widget\n\nA widget library.").await;

        let ctx = read_dir_context(dir.path().to_str().unwrap()).await;
        let prompt = ctx.prompt_block.unwrap();
        for name in DIR_CONTEXT_FILES {
            assert!(prompt.contains(&format!("--- {name} (excerpt) ---")));
        }
        assert!(prompt.contains("Use pnpm test."));
        let summary = ctx.fallback_summary.unwrap();
        assert!(summary.contains("CLAUDE.md: Guide — Use pnpm test."));
        assert!(summary.contains("README.md: Widget — A widget library."));
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS);
    }

    #[tokio::test]
    async fn symlinked_claude_md_is_included_once() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "AGENTS.md", "# Agents\n\nShared guide.").await;
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("AGENTS.md"), dir.path().join("CLAUDE.md"))
            .unwrap();

        let ctx = read_dir_context(dir.path().to_str().unwrap()).await;
        let prompt = ctx.prompt_block.unwrap();
        #[cfg(unix)]
        {
            assert_eq!(prompt.matches("Shared guide.").count(), 1);
            // The first name in probe order wins.
            assert!(prompt.contains("--- CLAUDE.md (excerpt) ---"));
            assert!(!prompt.contains("--- AGENTS.md (excerpt) ---"));
        }
        #[cfg(not(unix))]
        assert!(prompt.contains("--- AGENTS.md (excerpt) ---"));
    }

    #[tokio::test]
    async fn missing_files_and_bogus_dir_yield_empty_context() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = read_dir_context(dir.path().to_str().unwrap()).await;
        assert!(ctx.prompt_block.is_none());
        assert!(ctx.fallback_summary.is_none());

        let ctx = read_dir_context("/no/such/workspace/anywhere").await;
        assert!(ctx.prompt_block.is_none());
        assert!(ctx.fallback_summary.is_none());
    }

    #[tokio::test]
    async fn empty_and_whitespace_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "README.md", "  \n\n  ").await;
        let ctx = read_dir_context(dir.path().to_str().unwrap()).await;
        assert!(ctx.prompt_block.is_none());
        assert!(ctx.fallback_summary.is_none());
    }

    #[tokio::test]
    async fn long_file_excerpt_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!("# Big\n\n{}", "word ".repeat(3_000));
        write(dir.path(), "README.md", &long).await;
        let ctx = read_dir_context(dir.path().to_str().unwrap()).await;
        let prompt = ctx.prompt_block.unwrap();
        let excerpt = prompt.split("---\n").nth(1).unwrap();
        assert!(excerpt.chars().count() <= MAX_FILE_EXCERPT_CHARS);
        assert!(excerpt.trim_end().ends_with('…'));
    }

    #[test]
    fn digest_skips_frontmatter_fences_and_badges() {
        let text = "---\ntitle: X\n---\n\n[![CI](img)](link)\n\n# Tool\n\n```sh\nmake\n```\n\nFirst prose line\ncontinues here.\n\nSecond paragraph.";
        let digest = digest_markdown(text).unwrap();
        assert_eq!(digest, "Tool — First prose line continues here.");
    }

    #[test]
    fn digest_of_heading_only_file_is_the_heading() {
        assert_eq!(
            digest_markdown("# Just A Title").as_deref(),
            Some("Just A Title")
        );
        assert!(digest_markdown("```\ncode only\n```").is_none());
    }

    #[test]
    fn truncate_chars_is_utf8_safe_and_marks_the_cut() {
        assert_eq!(truncate_chars("short", 10), "short");
        let cut = truncate_chars(&"é".repeat(20), 10);
        assert!(cut.chars().count() <= 10);
        assert!(cut.ends_with('…'));
    }
}
