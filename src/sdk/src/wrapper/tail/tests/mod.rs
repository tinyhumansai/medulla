//! Tests for the tail module.

use super::*;
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

mod types;
use types::Fixture;

impl Fixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "medulla-tail-{}-{}-{id}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let codex_dir = dir.join("codex");
        let cwd = dir.join("work");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let mut env = HashMap::new();
        env.insert(
            "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
            codex_dir.to_string_lossy().into_owned(),
        );
        // Steer the claude dir somewhere empty so it never interferes.
        env.insert(
            "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
            dir.join("claude-empty").to_string_lossy().into_owned(),
        );
        Fixture {
            dir,
            codex_dir,
            env,
            cwd: cwd.to_string_lossy().into_owned(),
        }
    }

    fn meta_line(&self, id: &str) -> String {
        serde_json::json!({
            "type": "session_meta",
            "payload": { "session_id": id, "cwd": self.cwd }
        })
        .to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn agent_message(text: &str) -> String {
    serde_json::json!({
        "type": "event_msg",
        "timestamp": "2026-07-05T00:00:00.000Z",
        "payload": { "type": "agent_message", "message": text }
    })
    .to_string()
}

#[test]
fn resuming_starts_at_the_end_of_an_existing_transcript() {
    // A session being *reused* has a transcript that already exists and is
    // older than this turn — exactly what `new` is built to ignore. Without
    // `resuming` the turn never locates anything and reports that the
    // harness never started; located at byte zero it would be worse, because
    // the previous turn's completion record is still in the file and the
    // fold would settle on it and answer the wrong question.
    let fx = Fixture::new();
    let path = fx.codex_dir.join("rollout-live.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{}", fx.meta_line("sess-live")).unwrap();
    writeln!(file, "{}", agent_message("answer to the previous turn")).unwrap();
    file.flush().unwrap();

    // `new` alone cannot see it: pre-existing, and older than the start.
    let mut fresh = SessionTailer::new(
        fx.env.clone(),
        SessionAgentKind::Codex,
        fx.cwd.clone(),
        crate::clock::now_millis(),
    )
    .expecting("sess-live");
    assert!(
        fresh.poll().located.is_none(),
        "a fresh tailer must not adopt a transcript that predates it"
    );

    let mut resumed = SessionTailer::new(
        fx.env.clone(),
        SessionAgentKind::Codex,
        fx.cwd.clone(),
        crate::clock::now_millis(),
    )
    .resuming("sess-live");

    let first = resumed.poll();
    assert!(first.located.is_some(), "the live transcript must be found");
    assert!(
        first.lines.is_empty(),
        "history is already answered; only what comes next is this turn's: {:?}",
        first.lines
    );

    writeln!(file, "{}", agent_message("answer to this turn")).unwrap();
    file.flush().unwrap();

    let next = resumed.poll();
    let texts: Vec<&str> = next.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts.len(), 1, "got {texts:?}");
    assert!(texts[0].contains("answer to this turn"), "got {texts:?}");
}

#[test]
fn locates_new_file_and_streams_appended_lines() {
    let fx = Fixture::new();
    let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
    // Nothing yet.
    assert!(tailer.poll().located.is_none());

    // The child creates its transcript.
    let path = fx.codex_dir.join("rollout-abc.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{}", fx.meta_line("codex-1")).unwrap();
    writeln!(file, "{}", agent_message("first")).unwrap();
    file.flush().unwrap();

    let poll = tailer.poll();
    let located = poll.located.expect("transcript located");
    assert_eq!(located.harness_session_id, "codex-1");
    assert_eq!(located.cwd.as_deref(), Some(fx.cwd.as_str()));
    // Two complete lines (meta + message).
    assert_eq!(poll.lines.len(), 2);
    assert_eq!(poll.lines[0].line_no, 1);
    assert_eq!(poll.lines[1].line_no, 2);
    assert!(poll.lines[1].text.contains("first"));

    // Append more; only the new line comes back.
    writeln!(file, "{}", agent_message("second")).unwrap();
    file.flush().unwrap();
    let poll = tailer.poll();
    assert!(poll.located.is_none(), "already located");
    assert_eq!(poll.lines.len(), 1);
    assert_eq!(poll.lines[0].line_no, 3);
    assert!(poll.lines[0].text.contains("second"));
}

#[test]
fn holds_partial_line_until_newline_arrives() {
    let fx = Fixture::new();
    let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
    let path = fx.codex_dir.join("rollout-partial.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{}", fx.meta_line("codex-2")).unwrap();
    // Write a line with no trailing newline yet.
    write!(file, "{}", agent_message("incomplete")).unwrap();
    file.flush().unwrap();

    let poll = tailer.poll();
    assert!(poll.located.is_some());
    // Only the terminated meta line surfaces; the partial is buffered.
    assert_eq!(poll.lines.len(), 1);

    // Finish the line.
    writeln!(file).unwrap();
    file.flush().unwrap();
    let poll = tailer.poll();
    assert_eq!(poll.lines.len(), 1);
    assert!(poll.lines[0].text.contains("incomplete"));
    assert_eq!(poll.lines[0].line_no, 2);
}

#[test]
fn ignores_preexisting_transcripts() {
    let fx = Fixture::new();
    // A transcript that exists before the tailer starts.
    let old = fx.codex_dir.join("rollout-old.jsonl");
    let mut file = fs::File::create(&old).unwrap();
    writeln!(file, "{}", fx.meta_line("codex-old")).unwrap();
    file.flush().unwrap();

    let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
    // Even after a poll, the pre-existing file is not latched.
    assert!(tailer.poll().located.is_none());
}

#[test]
fn resets_on_truncation() {
    let fx = Fixture::new();
    let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
    let path = fx.codex_dir.join("rollout-rot.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{}", fx.meta_line("codex-3")).unwrap();
    writeln!(file, "{}", agent_message("aaaaaaaaaa")).unwrap();
    writeln!(file, "{}", agent_message("bbbbbbbbbb")).unwrap();
    file.flush().unwrap();
    let poll = tailer.poll();
    assert_eq!(poll.lines.len(), 3);

    // Truncate the file to strictly fewer bytes and write fresh content.
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file, "{}", fx.meta_line("codex-3")).unwrap();
    writeln!(file, "{}", agent_message("c")).unwrap();
    file.flush().unwrap();
    let poll = tailer.poll();
    // The tailer detects the shrink and re-reads from the top.
    assert_eq!(poll.lines.len(), 2);
    assert!(poll.lines[1].text.contains("\"c\""));
    assert_eq!(poll.lines[0].line_no, 1, "line numbering restarts");
}
