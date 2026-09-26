//! Transcript round-tripping, the replay bound, and the collision guard.

use tinyinference::message::Message;

use super::{load, save, trim, MAX_REPLAYED};

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn a_thread_with_no_history_replays_nothing() {
    let dir = home();
    assert!(load(dir.path(), "medulla-fresh").is_empty());
}

#[test]
fn a_saved_transcript_comes_back_in_order() {
    let dir = home();
    let messages = vec![
        Message::user("first"),
        Message::assistant("second"),
        Message::user("third"),
    ];
    save(dir.path(), "t-1", &messages);

    let loaded = load(dir.path(), "t-1");
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].text(), "first");
    assert_eq!(loaded[2].text(), "third");
}

/// Every replayed message is prompt budget spent before the model reads the new
/// instruction, so a long conversation replays its tail rather than all of it.
#[test]
fn a_long_transcript_replays_only_its_tail() {
    let dir = home();
    let messages: Vec<Message> = (0..MAX_REPLAYED + 40)
        .map(|i| Message::user(format!("m{i}")))
        .collect();
    save(dir.path(), "t-long", &messages);

    let loaded = load(dir.path(), "t-long");
    assert!(loaded.len() <= MAX_REPLAYED);
    assert_eq!(
        loaded.last().unwrap().text(),
        format!("m{}", MAX_REPLAYED + 39),
        "the newest message must survive"
    );
}

/// A tool exchange is three messages that must arrive together. Cutting between
/// the assistant's request and its result leaves an orphaned `tool` message,
/// which an OpenAI-compatible endpoint rejects outright — so the resumed turn
/// would fail on the wire instead of continuing.
#[test]
fn trimming_never_leaves_an_orphaned_tool_result() {
    let mut messages = vec![Message::user("start")];
    for i in 0..40 {
        messages.push(Message::assistant(format!("calling {i}")));
        messages.push(Message::tool(format!("call-{i}"), format!("result {i}")));
        messages.push(Message::user(format!("next {i}")));
    }

    for max in [1usize, 2, 3, 5, 8, 13, 40, 80] {
        let out = trim(messages.clone(), max);
        assert!(
            !matches!(out.first(), Some(Message::Tool(_))),
            "max={max} left a leading tool result"
        );
        assert!(out.len() <= max.max(out.len().min(max)), "max={max}");
    }
}

/// A window containing no user message has no cut a provider would accept, so
/// nothing is replayed rather than a malformed prefix that fails the turn.
#[test]
fn a_window_with_no_user_turn_replays_nothing() {
    let messages = vec![
        Message::user("only at the very start"),
        Message::assistant("a"),
        Message::tool("call-1", "r"),
        Message::assistant("b"),
    ];
    assert!(trim(messages, 2).is_empty());
}

/// A short, already-valid transcript is returned untouched.
#[test]
fn a_short_transcript_is_not_trimmed() {
    let messages = vec![Message::user("hi"), Message::assistant("hello")];
    assert_eq!(trim(messages.clone(), MAX_REPLAYED).len(), messages.len());
}

/// Trimmed on write too, or a thread's file grows forever just because reads
/// happen to cap it.
#[test]
fn the_stored_file_is_trimmed_not_just_the_replay() {
    let dir = home();
    let messages: Vec<Message> = (0..MAX_REPLAYED * 3)
        .map(|i| Message::user(format!("m{i}")))
        .collect();
    save(dir.path(), "t-big", &messages);

    let path = dir.path().join("agent-threads").join("t-big.json");
    let body = std::fs::read(&path).unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        decoded["messages"].as_array().unwrap().len() <= MAX_REPLAYED,
        "the file itself must be bounded"
    );
}

/// A transcript holds the operator's prompts, file contents the turn read, and
/// tool output. On a shared host that is workspace data other local users have
/// no business reading, even though it is not a bearer token.
#[cfg(unix)]
#[test]
fn transcripts_and_their_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = home();
    save(dir.path(), "t-perm", &[Message::user("secret prompt")]);

    let threads = dir.path().join("agent-threads");
    let file = threads.join("t-perm.json");
    let dir_mode = std::fs::metadata(&threads).unwrap().permissions().mode();
    let file_mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700, "directory is {dir_mode:o}");
    assert_eq!(file_mode & 0o777, 0o600, "file is {file_mode:o}");
}

/// Sanitizing an id for use as a filename can map two ids onto one file. The id
/// is recorded inside and checked on read, so a collision reads as "no history"
/// rather than as another conversation's transcript.
#[test]
fn a_filename_collision_does_not_serve_another_threads_history() {
    let dir = home();
    save(dir.path(), "a/b", &[Message::user("private")]);

    // `a/b` and `a:b` both sanitize to `a_b`.
    let loaded = load(dir.path(), "a:b");
    assert!(
        loaded.is_empty(),
        "a colliding id must not read another thread's messages"
    );
    // The original id still reads its own.
    assert_eq!(load(dir.path(), "a/b").len(), 1);
}

/// A corrupt cache must not fail the turn: answering without context beats
/// refusing to answer.
#[test]
fn an_unreadable_transcript_reads_as_no_history() {
    let dir = home();
    let path = dir.path().join("agent-threads");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("t-bad.json"), b"{ not json").unwrap();
    assert!(load(dir.path(), "t-bad").is_empty());
}
