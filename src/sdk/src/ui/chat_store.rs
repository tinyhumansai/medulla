//! On-disk chat persistence for the Chat tab's thread trees.
//!
//! Layout — one folder per MAIN chat (the root thread):
//!   <chatsDir>/<mainSessionId>/
//!     tree.json         structure (names, forkPoints, turn counts, md filenames)
//!     <sessionId>.md    one file per thread, turns framed with HTML comments
//!
//! Each `.md` delimits turns with an own-line marker `<!-- turn:user -->` that
//! renders invisibly but is unmistakable to the parser, so a reply full of
//! `## headings` or code fences round-trips losslessly. Content lines that would
//! read as a marker are backslash-escaped on write, restored on read.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const ROLES: &[&str] = &["system", "user", "assistant", "tool"];

/// A conversation turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// An in-memory tree node (messages included on save; loaded from disk on load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatNode {
    pub session_id: String,
    pub name: String,
    pub fork_point: Option<i64>,
    pub messages: Vec<ChatMessage>,
    pub children: Vec<ChatNode>,
}

/// One row for the `/resume` picker — from `tree.json` alone (no md reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainChatSummary {
    pub session_id: String,
    pub name: String,
    pub turns: usize,
    pub thread_count: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredNode {
    #[serde(rename = "sessionId")]
    session_id: String,
    name: String,
    #[serde(rename = "forkPoint", skip_serializing_if = "Option::is_none", default)]
    fork_point: Option<i64>,
    turns: usize,
    md: String,
    children: Vec<StoredNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTree {
    version: u8,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    root: StoredNode,
}

fn is_marker_line(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("<!-- turn:")?.strip_suffix(" -->")?;
    if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(inner)
    } else {
        None
    }
}

fn is_escaped_marker_line(line: &str) -> bool {
    line.strip_prefix('\\').and_then(is_marker_line).is_some()
}

/// Backslash-escape any own-line marker inside content so archived text can't
/// forge a turn on resume.
fn escape_turns(content: &str) -> String {
    content
        .split('\n')
        .map(|line| {
            if is_marker_line(line).is_some() {
                format!("\\{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unescape_turns(content: &str) -> String {
    content
        .split('\n')
        .map(|line| {
            if is_escaped_marker_line(line) {
                line[1..].to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a thread's messages as delimited markdown. Framing is exactly
/// `<marker>\n<content>\n` per turn, concatenated with no extra separator.
pub fn serialize_md(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        out.push_str(&format!(
            "<!-- turn:{} -->\n{}\n",
            m.role,
            escape_turns(&m.content)
        ));
    }
    out
}

/// Parse delimited markdown back into messages — the exact inverse of
/// [`serialize_md`]. Strips only the single leading + trailing framing newline,
/// restores escaped markers, and drops turns with an unknown role.
pub fn parse_md(md: &str) -> Vec<ChatMessage> {
    // Reproduce the JS capturing-split: alternating [preamble, role, body, ...].
    // Markers are own-line, so we walk lines and split on marker lines.
    let mut parts: Vec<String> = Vec::new(); // preamble, role, body, role, body...
    let mut current = String::new();
    let mut first_line = true;
    for line in md.split('\n') {
        if let Some(role) = is_marker_line(line) {
            // Flush the accumulated text segment (drop the trailing '\n' this
            // marker line's boundary contributed — see reconstruction below).
            parts.push(std::mem::take(&mut current));
            parts.push(role.to_string());
            first_line = true;
            continue;
        }
        if !first_line {
            current.push('\n');
        }
        current.push_str(line);
        first_line = false;
    }
    parts.push(current);

    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < parts.len() {
        let role = &parts[i];
        let body = &parts[i + 1];
        i += 2;
        if !ROLES.contains(&role.as_str()) {
            continue;
        }
        // The body segment carries a leading '\n' (from after the marker) and a
        // trailing '\n' (before the next marker / EOF); strip exactly one each.
        let trimmed = body
            .strip_prefix('\n')
            .unwrap_or(body)
            .strip_suffix('\n')
            .unwrap_or_else(|| body.strip_prefix('\n').unwrap_or(body));
        out.push(ChatMessage {
            role: role.clone(),
            content: unescape_turns(trimmed),
        });
    }
    out
}

fn count_turns(messages: &[ChatMessage]) -> usize {
    messages.len().div_ceil(2)
}

fn to_stored(node: &ChatNode) -> StoredNode {
    StoredNode {
        session_id: node.session_id.clone(),
        name: node.name.clone(),
        fork_point: node.fork_point,
        turns: count_turns(&node.messages),
        md: format!("{}.md", node.session_id),
        children: node.children.iter().map(to_stored).collect(),
    }
}

fn write_md_recursive(chat_dir: &Path, node: &ChatNode) -> io::Result<()> {
    fs::write(
        chat_dir.join(format!("{}.md", node.session_id)),
        serialize_md(&node.messages),
    )?;
    for child in &node.children {
        write_md_recursive(chat_dir, child)?;
    }
    Ok(())
}

/// Persist a whole chat tree: `tree.json` + one `.md` per thread, keyed by the
/// root's sessionId (re-saving the same root overwrites in place).
pub fn save_chat_tree(chats_dir: &Path, root: &ChatNode, now_millis: i64) -> io::Result<()> {
    let chat_dir = chats_dir.join(&root.session_id);
    fs::create_dir_all(&chat_dir)?;
    write_md_recursive(&chat_dir, root)?;
    let tree = StoredTree {
        version: 1,
        updated_at: iso8601_utc(now_millis),
        root: to_stored(root),
    };
    let json = serde_json::to_string_pretty(&tree).map_err(io::Error::other)?;
    fs::write(chat_dir.join("tree.json"), json)
}

fn count_nodes(node: &StoredNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

/// List every saved MAIN chat, newest first. Reads only `tree.json` per chat;
/// unreadable/half-written dirs are skipped.
pub fn list_main_chats(chats_dir: &Path) -> Vec<MainChatSummary> {
    let entries = match fs::read_dir(chats_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("tree.json");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(tree) = serde_json::from_str::<StoredTree>(&text) else {
            continue;
        };
        out.push(MainChatSummary {
            session_id: tree.root.session_id.clone(),
            name: tree.root.name.clone(),
            turns: tree.root.turns,
            thread_count: count_nodes(&tree.root),
            updated_at: tree.updated_at,
        });
    }
    // ISO-8601 UTC strings sort lexicographically = chronologically.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// Rebuild a saved chat tree with each thread's messages loaded from its `.md`.
/// Returns `None` when the chat has no readable `tree.json`; a missing per-thread
/// `.md` degrades to an empty message list rather than failing the load.
pub fn load_chat_tree(chats_dir: &Path, main_session_id: &str) -> Option<ChatNode> {
    let chat_dir = chats_dir.join(main_session_id);
    let text = fs::read_to_string(chat_dir.join("tree.json")).ok()?;
    let tree: StoredTree = serde_json::from_str(&text).ok()?;
    Some(load_node(&chat_dir, &tree.root))
}

fn load_node(chat_dir: &Path, node: &StoredNode) -> ChatNode {
    let messages = fs::read_to_string(chat_dir.join(&node.md))
        .map(|md| parse_md(&md))
        .unwrap_or_default();
    ChatNode {
        session_id: node.session_id.clone(),
        name: node.name.clone(),
        fork_point: node.fork_point,
        messages,
        children: node
            .children
            .iter()
            .map(|c| load_node(chat_dir, c))
            .collect(),
    }
}

/// Current wall-clock in epoch millis, for [`save_chat_tree`].
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Format epoch millis as an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS.sssZ`).
pub fn iso8601_utc(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let ms = millis.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z")
}

/// Howard Hinnant's days-from-civil, inverted: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
        }
    }

    #[test]
    fn serialize_parse_round_trip() {
        let messages = vec![
            msg("user", "hello\n## heading\n```code```"),
            msg("assistant", "reply with\nnewlines\n"),
        ];
        let md = serialize_md(&messages);
        assert_eq!(parse_md(&md), messages);
    }

    #[test]
    fn marker_lookalike_is_escaped() {
        let messages = vec![msg("user", "<!-- turn:assistant -->\nnot a real turn")];
        let md = serialize_md(&messages);
        assert!(md.contains("\\<!-- turn:assistant -->"));
        let parsed = parse_md(&md);
        assert_eq!(parsed, messages);
        // Exactly one turn — the escaped line did not forge a boundary.
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn unknown_role_dropped() {
        let md = "<!-- turn:robot -->\nhi\n<!-- turn:user -->\nreal\n";
        let parsed = parse_md(md);
        assert_eq!(parsed, vec![msg("user", "real")]);
    }

    #[test]
    fn tree_save_list_load_round_trip() {
        let tmp = std::env::temp_dir().join(format!("medulla-chat-{}", now_millis()));
        let root = ChatNode {
            session_id: "root-1".into(),
            name: "Main".into(),
            fork_point: None,
            messages: vec![msg("user", "q"), msg("assistant", "a")],
            children: vec![ChatNode {
                session_id: "fork-1".into(),
                name: "Fork".into(),
                fork_point: Some(2),
                messages: vec![msg("user", "q2")],
                children: vec![],
            }],
        };
        save_chat_tree(&tmp, &root, 1_700_000_000_000).unwrap();
        let list = list_main_chats(&tmp);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, "root-1");
        assert_eq!(list[0].turns, 1);
        assert_eq!(list[0].thread_count, 2);
        let loaded = load_chat_tree(&tmp, "root-1").unwrap();
        assert_eq!(loaded, root);
        assert!(load_chat_tree(&tmp, "missing").is_none());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn iso_format_is_stable() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
    }
}
