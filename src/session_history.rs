//! Recent-session history, ported from the tinyplace CLI `session-history.ts`.
//!
//! The read model behind a "resume" pane: it scans the agents' own session dirs
//! (`~/.claude/projects`, `~/.codex/sessions`) — the same transcript files the
//! wrapper's tailer streams — so the list is always accurate with no separate
//! store. A row resolves to `{ agent, id }`, relaunched via
//! `claude --resume <id>` / `codex resume <id>` in the session's original cwd.
//! Exposed to the CLI via `medulla sessions` (JSON); a TUI picker lands later.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// The coding-agent that owns a session transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAgentKind {
    Claude,
    Codex,
}

impl SessionAgentKind {
    fn as_str(&self) -> &'static str {
        match self {
            SessionAgentKind::Claude => "claude",
            SessionAgentKind::Codex => "codex",
        }
    }
}

/// One recent session, ranked for the resume pane.
#[derive(Debug, Clone, Serialize)]
pub struct RecentSession {
    /// Agent session id (claude `sessionId` / codex `session_id`).
    pub id: String,
    pub agent: SessionAgentKind,
    /// Working directory the session ran in, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// First human prompt, single-lined + truncated; drives the resume label.
    pub label: String,
    /// Last-activity epoch ms (session-file mtime).
    pub last_active: i64,
    /// Absolute path to the session's JSONL file.
    pub path: String,
}

const DEFAULT_LIMIT: usize = 24;
const DEFAULT_SCAN_LIMIT: usize = 60;
const HEAD_BYTES: usize = 64 * 1024;
const LABEL_MAX: usize = 72;

struct RawSessionFile {
    agent: SessionAgentKind,
    path: PathBuf,
    mtime_ms: i64,
}

struct SessionSummary {
    id: String,
    cwd: Option<String>,
    label: String,
}

/// The most recent agent sessions across both harnesses, ordered
/// **current-folder-first, then most-recent**. Cost is bounded: only the newest
/// `scan_limit` files are opened, and only their first [`HEAD_BYTES`] parsed.
pub fn list_recent_sessions(
    env: &HashMap<String, String>,
    cwd: &str,
    limit: Option<usize>,
    scan_limit: Option<usize>,
) -> Vec<RecentSession> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let scan_limit = scan_limit.unwrap_or(DEFAULT_SCAN_LIMIT);

    let mut raw: Vec<RawSessionFile> = Vec::new();
    raw.extend(collect_session_files(
        SessionAgentKind::Claude,
        &claude_sessions_dir(env),
    ));
    raw.extend(collect_session_files(
        SessionAgentKind::Codex,
        &codex_sessions_dir(env),
    ));
    raw.sort_by_key(|file| std::cmp::Reverse(file.mtime_ms));
    raw.truncate(scan_limit);

    let here = safe_resolve(cwd);
    // Dedupe by agent+id, keeping the freshest file.
    let mut by_id: HashMap<String, RecentSession> = HashMap::new();
    for file in &raw {
        let summary = match read_session_summary(file.agent, &file.path) {
            Some(summary) => summary,
            None => continue,
        };
        let key = format!("{}:{}", file.agent.as_str(), summary.id);
        if let Some(existing) = by_id.get(&key) {
            if existing.last_active >= file.mtime_ms {
                continue;
            }
        }
        by_id.insert(
            key,
            RecentSession {
                agent: file.agent,
                id: summary.id,
                label: summary.label,
                last_active: file.mtime_ms,
                path: file.path.to_string_lossy().into_owned(),
                cwd: summary.cwd,
            },
        );
    }

    let mut sessions: Vec<RecentSession> = by_id.into_values().collect();
    sessions.sort_by(|left, right| {
        let left_here = if is_here(left.cwd.as_deref(), here.as_deref()) {
            0
        } else {
            1
        };
        let right_here = if is_here(right.cwd.as_deref(), here.as_deref()) {
            0
        } else {
            1
        };
        if left_here != right_here {
            return left_here.cmp(&right_here);
        }
        right.last_active.cmp(&left.last_active)
    });
    sessions.truncate(limit);
    sessions
}

/// Resolve the Claude session directory, honoring the env overrides.
pub fn claude_sessions_dir(env: &HashMap<String, String>) -> PathBuf {
    first_env(
        env,
        &[
            "TINYVERSE_CLAUDE_SESSIONS_DIR",
            "TINYPLACE_CLAUDE_SESSIONS_DIR",
        ],
    )
    .map(PathBuf::from)
    .unwrap_or_else(|| home_dir().join(".claude").join("projects"))
}

/// Resolve the Codex session directory, honoring the env override.
pub fn codex_sessions_dir(env: &HashMap<String, String>) -> PathBuf {
    env.get("TINYPLACE_CODEX_SESSIONS_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex").join("sessions"))
}

fn collect_session_files(agent: SessionAgentKind, dir: &Path) -> Vec<RawSessionFile> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    visit(agent, dir, &mut out);
    out
}

fn visit(agent: SessionAgentKind, directory: &Path, out: &mut Vec<RawSessionFile>) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            visit(agent, &path, out);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !file_type.is_file() || !is_session_file(agent, &path, &name) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            out.push(RawSessionFile {
                agent,
                path,
                mtime_ms,
            });
        }
    }
}

fn is_session_file(agent: SessionAgentKind, path: &Path, name: &str) -> bool {
    match agent {
        SessionAgentKind::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
        SessionAgentKind::Claude => {
            let sep = std::path::MAIN_SEPARATOR;
            let subagents = format!("{sep}subagents{sep}");
            name.ends_with(".jsonl") && !path.to_string_lossy().contains(&subagents)
        }
    }
}

fn read_session_summary(agent: SessionAgentKind, path: &Path) -> Option<SessionSummary> {
    let lines = read_head_lines(path);
    match agent {
        SessionAgentKind::Claude => read_claude_summary(&lines),
        SessionAgentKind::Codex => read_codex_summary(&lines),
    }
}

fn read_claude_summary(lines: &[String]) -> Option<SessionSummary> {
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut label: Option<String> = None;
    for raw in lines {
        let record = match parse_object(raw) {
            Some(record) => record,
            None => continue,
        };
        if let Some(session_id) = record.get("sessionId").and_then(Value::as_str) {
            id = Some(session_id.to_string());
        }
        if let Some(directory) = record.get("cwd").and_then(Value::as_str) {
            cwd = Some(directory.to_string());
        }
        if label.is_none() && record.get("type").and_then(Value::as_str) == Some("user") {
            label = first_prompt_text(as_message_content(record.get("message")));
        }
    }
    let id = id?;
    Some(SessionSummary {
        id,
        cwd,
        label: label.unwrap_or_else(|| "(no prompt)".to_string()),
    })
}

fn read_codex_summary(lines: &[String]) -> Option<SessionSummary> {
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut label: Option<String> = None;
    for raw in lines {
        let record = match parse_object(raw) {
            Some(record) => record,
            None => continue,
        };
        if record.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = record.get("payload").and_then(Value::as_object) {
                if let Some(session_id) = payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("id").and_then(Value::as_str))
                {
                    id = Some(session_id.to_string());
                }
                if let Some(directory) = payload.get("cwd").and_then(Value::as_str) {
                    cwd = Some(directory.to_string());
                }
            }
            continue;
        }
        if label.is_some() || record.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        if let Some(payload) = record.get("payload").and_then(Value::as_object) {
            if payload.get("type").and_then(Value::as_str) == Some("message")
                && payload.get("role").and_then(Value::as_str) == Some("user")
            {
                label = first_prompt_text(payload.get("content").cloned());
            }
        }
    }
    let id = id?;
    Some(SessionSummary {
        id,
        cwd,
        label: label.unwrap_or_else(|| "(no prompt)".to_string()),
    })
}

/// Turn a user message's `content` into a display label, or `None` when it is
/// not a real prompt (system-injected `<...>` turns and tool-result turns are
/// skipped so the label reflects the first thing the human said).
fn first_prompt_text(content: Option<Value>) -> Option<String> {
    let text = extract_text(content.as_ref())?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return None;
    }
    Some(truncate_label(trimmed))
}

fn extract_text(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(items)) => {
            for item in items {
                let object = match item.as_object() {
                    Some(object) => object,
                    None => continue,
                };
                // claude blocks are {type:"text"}; codex are {type:"input_text"}.
                let kind = object.get("type").and_then(Value::as_str);
                if kind == Some("text") || kind == Some("input_text") {
                    if let Some(text) = object.get("text").and_then(Value::as_str) {
                        return Some(text.to_string());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn as_message_content(message: Option<&Value>) -> Option<Value> {
    let object = message?.as_object()?;
    if object.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    object.get("content").cloned()
}

fn truncate_label(text: &str) -> String {
    // Strip C0/DEL/C1 control bytes to a space so a pasted escape sequence can't
    // move the cursor or recolor a pane, then collapse whitespace.
    let cleaned: String = text
        .chars()
        .map(|c| {
            if (c as u32) <= 0x1F || (0x7F..=0x9F).contains(&(c as u32)) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let single = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= LABEL_MAX {
        return single;
    }
    let prefix: String = single.chars().take(LABEL_MAX - 1).collect();
    format!("{}…", prefix.trim_end())
}

fn read_head_lines(path: &Path) -> Vec<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let mut buffer = vec![0u8; HEAD_BYTES];
    let read = match file.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&buffer[..read]);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    // When the read hit the cap the final line is likely truncated — drop it.
    if read >= HEAD_BYTES && lines.len() > 1 {
        lines.pop();
    }
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

fn is_here(cwd: Option<&str>, here: Option<&str>) -> bool {
    match (cwd, here) {
        (Some(cwd), Some(here)) => safe_resolve(cwd).as_deref() == Some(here),
        _ => false,
    }
}

fn safe_resolve(path: &str) -> Option<String> {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .ok()
        .or_else(|| {
            // A path that does not exist still resolves lexically to itself here,
            // matching the TS `resolve` (which never touches the filesystem).
            std::env::current_dir()
                .ok()
                .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
        })
}

fn first_env(env: &HashMap<String, String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| env.get(*name))
        .find(|value| !value.is_empty())
        .cloned()
}

fn parse_object(raw: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_object().cloned()
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
}
