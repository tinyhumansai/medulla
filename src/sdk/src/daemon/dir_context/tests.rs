//! Tests for the dir context module.

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
    std::os::unix::fs::symlink(dir.path().join("AGENTS.md"), dir.path().join("CLAUDE.md")).unwrap();

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
