//! Tests for the log module.

use super::*;

#[test]
fn the_sink_captures_what_the_daemon_writes() {
    let buffer = LogBuffer::with_now(Arc::new(|| 42));
    let sink = buffer.sink();
    sink("task t1 → claude");
    sink("task t1 ✓ (12 events)");

    let lines = buffer.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text, "task t1 → claude");
    assert_eq!(lines[0].at, 42);
    assert_eq!(lines[1].text, "task t1 ✓ (12 events)");
}

#[test]
fn the_oldest_lines_are_dropped_once_full() {
    // A daemon left running for a week must not hold its whole history just
    // because nobody was watching.
    let buffer = LogBuffer::new();
    for i in 0..CAPACITY + 50 {
        buffer.push(format!("line {i}"));
    }
    assert_eq!(buffer.len(), CAPACITY);
    assert_eq!(buffer.lines()[0].text, "line 50", "oldest dropped first");
}

#[test]
fn tail_returns_the_most_recent_lines_oldest_first() {
    let buffer = LogBuffer::new();
    for i in 0..10 {
        buffer.push(format!("line {i}"));
    }
    let tail = buffer.tail(3);
    assert_eq!(
        tail.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["line 7", "line 8", "line 9"]
    );
}

#[test]
fn asking_for_more_than_exists_returns_everything() {
    let buffer = LogBuffer::new();
    buffer.push("only");
    assert_eq!(buffer.tail(100).len(), 1);
    assert!(LogBuffer::new().tail(10).is_empty());
}

#[test]
fn lines_are_mirrored_to_the_file() {
    // A screen only helps while someone is looking at it; the failures worth
    // chasing are usually found afterwards.
    let dir = tempfile::tempdir().unwrap();
    let buffer = LogBuffer::new();
    let path = buffer
        .attach_file(dir.path(), "worker")
        .expect("the file opens");

    buffer.push("task t1 → claude");
    buffer.push("task t1 ✗ provider exploded");

    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("task t1 → claude"));
    assert!(written.contains("provider exploded"));
    assert_eq!(written.lines().count(), 2);
    // Every line is timestamped, so a log read hours later is placeable.
    assert!(
        written.lines().all(|l| l.starts_with("20")),
        "got: {written}"
    );
}

#[test]
fn an_unwritable_directory_never_stops_logging() {
    // Logging must not be the reason a daemon fails to start.
    //
    // The unusable path is a directory *underneath a file*, which no
    // platform will create. It used to be `/proc/nonexistent/nope`, which is
    // only unusable where there is a `/proc` to speak of — on Windows that
    // is an ordinary relative path and the attach succeeded.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, b"x").unwrap();
    let buffer = LogBuffer::new();
    assert!(buffer.attach_file(&file.join("nope"), "worker").is_none());
    buffer.push("still recorded in memory");
    assert_eq!(buffer.len(), 1);
}

#[test]
fn an_oversized_log_is_rotated_rather_than_grown_forever() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("worker.log");
    std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();

    let buffer = LogBuffer::new();
    buffer.attach_file(dir.path(), "worker").expect("opens");
    buffer.push("fresh line");

    assert!(dir.path().join("worker.log.1").exists(), "previous kept");
    let current = std::fs::read_to_string(&path).unwrap();
    assert!(current.contains("fresh line"));
    assert!(current.len() < 200, "the live log restarted");
}

#[test]
fn the_log_directory_defaults_beside_the_identity_not_in_a_repo() {
    // A worker's workspace is full of the operator's real repositories;
    // dropping a log file into one invites it into a commit.
    let mut env = std::collections::HashMap::new();
    env.insert("MEDULLA_HOME".to_string(), "/tmp/mh".to_string());
    assert_eq!(default_log_dir(&env), std::path::Path::new("/tmp/mh/logs"));

    env.insert("MEDULLA_LOG_DIR".to_string(), "/tmp/elsewhere".to_string());
    assert_eq!(
        default_log_dir(&env),
        std::path::Path::new("/tmp/elsewhere"),
        "an explicit override wins"
    );
}

#[test]
fn clones_share_one_ring() {
    // The daemon writes through its sink while the render thread reads.
    let buffer = LogBuffer::new();
    let other = buffer.clone();
    other.push("from the clone");
    assert_eq!(buffer.len(), 1);
}

#[test]
fn the_default_buffer_is_an_empty_ring_on_the_system_clock() {
    // `Default` is what a struct field derives, so it must behave like the
    // named constructor rather than diverge silently.
    let buffer = LogBuffer::default();
    assert!(buffer.is_empty(), "a fresh buffer holds nothing");
    assert_eq!(buffer.len(), 0);
    buffer.push("first");
    assert!(!buffer.is_empty(), "a line makes it non-empty");
    assert_eq!(buffer.len(), 1);
}

#[test]
fn a_sink_that_can_no_longer_be_written_disables_itself_and_stops_trying() {
    // The file mirror is best-effort: a write error must disable the file
    // rather than propagate, and once disabled a later line must be dropped
    // silently instead of erroring again. Constructed directly because the
    // failure needs a handle that is open but not writable, which
    // `FileSink::open` (append mode) never produces.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("read-only.log");
    std::fs::write(&path, b"seed\n").unwrap();
    // Opened read-only: writing to it returns an error at the OS layer.
    let handle = OpenOptions::new().read(true).open(&path).unwrap();
    let mut sink = FileSink {
        path: path.clone(),
        handle: Some(handle),
    };

    sink.write("first attempt fails");
    assert!(
        sink.handle.is_none(),
        "a write error must disable the file, not surface"
    );
    // The second call takes the early return for an already-disabled handle.
    sink.write("second attempt is a silent no-op");

    // Nothing the failed sink was handed reached the file.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        on_disk, "seed\n",
        "no line was written through a dead handle"
    );
}
