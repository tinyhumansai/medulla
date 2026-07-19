//! Unit tests for recent-session scanning, summary parsing, label extraction,
//! and current-folder-first ranking.

use super::scan::{collect_session_files, is_here, is_session_file, sessions_dir_for};
use super::summary::{
    as_message_content, extract_text, first_prompt_text, read_claude_summary, read_codex_summary,
    truncate_label, LABEL_MAX,
};
use super::*;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

fn write_session(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn ranks_current_cwd_first_then_recency() {
    let tmp = std::env::temp_dir().join(format!("medulla-sh-{}", std::process::id()));
    let claude_dir = tmp.join("claude");
    let codex_dir = tmp.join("codex");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&codex_dir).unwrap();

    let here = tmp.join("workspace");
    fs::create_dir_all(&here).unwrap();
    let here_str = here.to_string_lossy().into_owned();

    // A session in a different cwd.
    write_session(
        &claude_dir,
        "a.jsonl",
        &format!(
            "{}\n",
            serde_json::json!({"sessionId":"claude-a","cwd":"/elsewhere","type":"user","message":{"role":"user","content":"do A"}})
        ),
    );
    // A session in the current cwd — ranks first regardless of recency.
    write_session(
        &codex_dir,
        "rollout-b.jsonl",
        &format!(
            "{}\n{}\n",
            serde_json::json!({"type":"session_meta","payload":{"session_id":"codex-b","cwd":here_str}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"do B here"}]}})
        ),
    );

    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        claude_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        codex_dir.to_string_lossy().into_owned(),
    );

    let sessions = list_recent_sessions(&env, &here_str, None, None);
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "codex-b", "current-cwd session ranks first");
    assert_eq!(sessions[0].agent, SessionAgentKind::Codex);
    assert_eq!(sessions[0].label, "do B here");
    assert_eq!(sessions[1].id, "claude-a");
    assert_eq!(sessions[1].label, "do A");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn skips_bracketed_system_prompts_for_label() {
    assert_eq!(
        first_prompt_text(Some(Value::String(
            "<command-name>foo</command-name>".into()
        ))),
        None
    );
    assert_eq!(
        first_prompt_text(Some(Value::String("real prompt".into()))).as_deref(),
        Some("real prompt")
    );
}

#[test]
fn label_strips_control_bytes_and_truncates() {
    let noisy = "hello\u{001b}[31m world \u{0007}".to_string();
    assert_eq!(truncate_label(&noisy), "hello [31m world");
    let long = "x".repeat(100);
    let label = truncate_label(&long);
    assert!(label.chars().count() <= LABEL_MAX);
    assert!(label.ends_with('…'));
}

#[test]
fn extract_text_from_string_and_blocks() {
    assert_eq!(
        extract_text(Some(&Value::String("plain".into()))).as_deref(),
        Some("plain")
    );
    // Claude text block.
    let claude = serde_json::json!([{"type":"text","text":"hello claude"}]);
    assert_eq!(extract_text(Some(&claude)).as_deref(), Some("hello claude"));
    // Codex input_text block.
    let codex = serde_json::json!([{"type":"input_text","text":"hello codex"}]);
    assert_eq!(extract_text(Some(&codex)).as_deref(), Some("hello codex"));
    // Unhandled shapes → None.
    assert_eq!(extract_text(Some(&serde_json::json!({"x":1}))), None);
    assert_eq!(extract_text(None), None);
    // A block array with no text block → None.
    let empty = serde_json::json!([{"type":"image"}]);
    assert_eq!(extract_text(Some(&empty)), None);
}

#[test]
fn first_prompt_text_rejects_empty_and_whitespace() {
    assert_eq!(first_prompt_text(Some(Value::String("   ".into()))), None);
    assert_eq!(first_prompt_text(None), None);
}

#[test]
fn collect_session_files_recurses_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    // A matching top-level file, a non-matching one, and a nested match.
    fs::write(dir.path().join("a.jsonl"), "{}").unwrap();
    fs::write(dir.path().join("notes.txt"), "x").unwrap();
    let nested = dir.path().join("deep").join("er");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("b.jsonl"), "{}").unwrap();
    // A subagents dir is excluded for claude transcripts.
    let subagents = dir.path().join("subagents");
    fs::create_dir_all(&subagents).unwrap();
    fs::write(subagents.join("c.jsonl"), "{}").unwrap();

    let files = collect_session_files(SessionAgentKind::Claude, dir.path());
    let names: Vec<String> = files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"a.jsonl".to_string()));
    assert!(names.contains(&"b.jsonl".to_string()));
    assert!(!names.contains(&"c.jsonl".to_string()));
    assert!(!names.contains(&"notes.txt".to_string()));

    // An absent directory yields nothing.
    assert!(collect_session_files(SessionAgentKind::Claude, &dir.path().join("nope")).is_empty());
}

#[test]
fn sessions_dir_for_honors_env_override() {
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        "/tmp/custom-claude".to_string(),
    );
    assert_eq!(
        sessions_dir_for(&env, SessionAgentKind::Claude),
        PathBuf::from("/tmp/custom-claude")
    );
}

#[test]
fn discover_newest_session_file_matches_cwd_and_skips_old_and_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        dir.path().to_string_lossy().into_owned(),
    );

    // A session recorded in a different cwd is skipped; one with no cwd matches.
    write_session(
        dir.path(),
        "wrong.jsonl",
        &serde_json::json!({"sessionId":"wrong","cwd":"/somewhere/else","type":"user","message":{"role":"user","content":"x"}}).to_string(),
    );
    let matching = write_session(
        dir.path(),
        "match.jsonl",
        &serde_json::json!({"sessionId":"match-1","type":"user","message":{"role":"user","content":"hi"}}).to_string(),
    );

    let ignored = std::collections::HashSet::new();
    let found = discover_newest_session_file(
        &env,
        SessionAgentKind::Claude,
        "/does/not/matter",
        0,
        &ignored,
    )
    .expect("a cwd-less session should be discovered");
    assert_eq!(found.id, "match-1");

    // Ignoring the matching file leaves nothing to discover.
    let mut ignored = std::collections::HashSet::new();
    ignored.insert(std::fs::canonicalize(&matching).unwrap());
    assert!(
        discover_newest_session_file(&env, SessionAgentKind::Claude, "/x", 0, &ignored).is_none()
    );

    // A min_mtime far in the future skips every file.
    assert!(discover_newest_session_file(
        &env,
        SessionAgentKind::Claude,
        "/x",
        i64::MAX,
        &std::collections::HashSet::new()
    )
    .is_none());
}

#[test]
fn is_here_needs_both_sides() {
    assert!(!is_here(None, Some("/x")));
    assert!(!is_here(Some("/x"), None));
}

#[test]
fn as_message_content_only_for_user_role() {
    let user = serde_json::json!({"role":"user","content":"hi"});
    assert_eq!(
        as_message_content(Some(&user)),
        Some(Value::String("hi".into()))
    );
    let assistant = serde_json::json!({"role":"assistant","content":"hi"});
    assert_eq!(as_message_content(Some(&assistant)), None);
}

#[test]
fn codex_summary_uses_id_fallback_and_no_prompt_label() {
    // No `session_id`, only `id`; and no user message → "(no prompt)".
    let lines = vec![serde_json::json!({
        "type":"session_meta",
        "payload":{"id":"codex-x","cwd":"/here"}
    })
    .to_string()];
    let summary = read_codex_summary(&lines).unwrap();
    assert_eq!(summary.id, "codex-x");
    assert_eq!(summary.cwd.as_deref(), Some("/here"));
    assert_eq!(summary.label, "(no prompt)");
}

#[test]
fn codex_summary_without_meta_is_none() {
    let lines = vec![serde_json::json!({"type":"response_item"}).to_string()];
    assert!(read_codex_summary(&lines).is_none());
}

#[test]
fn claude_summary_without_session_id_is_none() {
    let lines = vec![
        serde_json::json!({"type":"user","message":{"role":"user","content":"hi"}}).to_string(),
    ];
    assert!(read_claude_summary(&lines).is_none());
}

#[test]
fn session_file_matching_rules() {
    let claude_ok = Path::new("/x/proj/abc.jsonl");
    assert!(is_session_file(
        SessionAgentKind::Claude,
        claude_ok,
        "abc.jsonl"
    ));
    // A subagents transcript is excluded.
    let sep = std::path::MAIN_SEPARATOR;
    let sub = PathBuf::from(format!("/x{sep}subagents{sep}abc.jsonl"));
    assert!(!is_session_file(
        SessionAgentKind::Claude,
        &sub,
        "abc.jsonl"
    ));
    // Codex requires the rollout- prefix.
    let codex_ok = Path::new("/x/rollout-1.jsonl");
    assert!(is_session_file(
        SessionAgentKind::Codex,
        codex_ok,
        "rollout-1.jsonl"
    ));
    assert!(!is_session_file(
        SessionAgentKind::Codex,
        Path::new("/x/other.jsonl"),
        "other.jsonl"
    ));
}

#[test]
fn agent_kind_as_str() {
    assert_eq!(SessionAgentKind::Claude.as_str(), "claude");
    assert_eq!(SessionAgentKind::Codex.as_str(), "codex");
}

#[test]
fn missing_dirs_yield_no_sessions() {
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        "/no/such/claude/dir".to_string(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        "/no/such/codex/dir".to_string(),
    );
    let sessions = list_recent_sessions(&env, "/tmp", None, None);
    assert!(sessions.is_empty());
}

#[test]
fn env_dir_overrides_resolve() {
    let mut env = HashMap::new();
    env.insert(
        "TINYVERSE_CLAUDE_SESSIONS_DIR".to_string(),
        "/custom/claude".to_string(),
    );
    assert_eq!(claude_sessions_dir(&env), PathBuf::from("/custom/claude"));
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        "/custom/codex".to_string(),
    );
    assert_eq!(codex_sessions_dir(&env), PathBuf::from("/custom/codex"));
    // Empty values are ignored (fall through to the home default).
    let mut empty = HashMap::new();
    empty.insert("TINYPLACE_CODEX_SESSIONS_DIR".to_string(), String::new());
    assert!(codex_sessions_dir(&empty).ends_with("sessions"));
}

#[test]
fn dedupe_keeps_the_freshest_file_for_an_id() {
    let tmp = std::env::temp_dir().join(format!("medulla-dedupe-{}", std::process::id()));
    let claude_dir = tmp.join("claude");
    fs::create_dir_all(&claude_dir).unwrap();
    // Two files, same sessionId; the newer one (by mtime) wins its label.
    let old = write_session(
        &claude_dir,
        "old.jsonl",
        &format!(
            "{}\n",
            serde_json::json!({"sessionId":"dup","cwd":"/x","type":"user","message":{"role":"user","content":"old label"}})
        ),
    );
    let new = write_session(
        &claude_dir,
        "new.jsonl",
        &format!(
            "{}\n",
            serde_json::json!({"sessionId":"dup","cwd":"/x","type":"user","message":{"role":"user","content":"new label"}})
        ),
    );
    let _ = (&old, &new);

    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        claude_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        tmp.join("codex").to_string_lossy().into_owned(),
    );
    let sessions = list_recent_sessions(&env, "/tmp", None, None);
    assert_eq!(sessions.len(), 1, "the two files dedupe to one session");
    let _ = fs::remove_dir_all(&tmp);
}
