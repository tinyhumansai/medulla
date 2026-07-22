//! [`LogBuffer`] — the daemon's log lines, captured for the screen instead of
//! scrolling past on stderr.
//!
//! The headless daemon already narrates itself through an injectable sink
//! ([`LogFn`](medulla::daemon::LogFn)), which `medulla daemon` points at
//! `eprintln!`. The worker TUI points it here instead, so the same lines an
//! operator reads in a normal terminal become the pane they read in the UI —
//! the same information, not a second, divergent rendering of it.
//!
//! Bounded on purpose: a daemon left running for a week must not accumulate its
//! entire history in memory just because nobody was looking at the screen.
//!
//! It also mirrors to a file when one is attached. A screen only helps while you
//! are looking at it: the failures worth chasing — a relay refusing a call, a
//! task erroring at 3am — are usually discovered afterwards, and an in-memory
//! ring that dies with the process cannot answer for them.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use medulla::daemon::LogFn;

/// How many lines to retain before dropping the oldest.
pub const CAPACITY: usize = 2_000;

/// Rotate a log once it passes this size, so a long-lived daemon cannot fill a
/// disk unattended.
pub const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Where logs are written when nothing overrides it.
///
/// Deliberately **not** the workspace: a worker's workspace is a directory full
/// of the operator's real repositories, and dropping a log file into one invites
/// it into a commit. `<medulla_home>/logs` is one predictable place, survives
/// changing the workspace, and works for the orchestrator, which has no
/// workspace at all. `MEDULLA_LOG_DIR` overrides it.
pub fn default_log_dir(env: &std::collections::HashMap<String, String>) -> PathBuf {
    if let Some(dir) = env
        .get("MEDULLA_LOG_DIR")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(dir);
    }
    medulla::home::medulla_home(env).join("logs")
}

/// A file every log line is appended to.
struct FileSink {
    path: PathBuf,
    handle: Option<File>,
}

impl FileSink {
    /// Open `dir/<name>.log` for appending, rotating an oversized existing file.
    ///
    /// Best-effort throughout: logging must never be the reason a daemon fails
    /// to start, so an unwritable directory disables the file and leaves the
    /// in-memory ring working.
    fn open(dir: &Path, name: &str) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(format!("{name}.log"));
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
            // One generation back is enough to cover a crash; more would just be
            // disk nobody reads.
            let _ = std::fs::rename(&path, dir.join(format!("{name}.log.1")));
        }
        let handle = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        Some(FileSink {
            path,
            handle: Some(handle),
        })
    }

    /// Append one timestamped line, giving up silently on error.
    fn write(&mut self, text: &str) {
        let Some(handle) = self.handle.as_mut() else {
            return;
        };
        // Flushed per line rather than buffered: a log that loses its last
        // writes on a crash omits exactly the lines that explain the crash.
        if writeln!(handle, "{} {text}", medulla::clock::iso_now()).is_err()
            || handle.flush().is_err()
        {
            self.handle = None;
        }
    }
}

/// One captured log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Epoch ms when it was written.
    pub at: i64,
    /// The line itself, as the daemon wrote it.
    pub text: String,
}

/// A bounded, shared ring of daemon log lines.
///
/// Cheap to clone; every clone reads and writes the same ring.
#[derive(Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    file: Arc<Mutex<Option<FileSink>>>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::new()
    }
}

impl LogBuffer {
    /// An empty buffer on the system clock.
    pub fn new() -> Self {
        LogBuffer {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(64))),
            now: Arc::new(medulla::clock::now_millis),
            file: Arc::new(Mutex::new(None)),
        }
    }

    /// Mirror every line to `dir/<name>.log`.
    ///
    /// Returns the path when the file could be opened, so a caller can tell the
    /// operator where to look. `None` means logging stays in memory only —
    /// never a startup failure.
    pub fn attach_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let sink = FileSink::open(dir, name)?;
        let path = sink.path.clone();
        *self.file.lock().unwrap() = Some(sink);
        Some(path)
    }

    /// Override the clock (tests).
    pub fn with_now(now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        LogBuffer {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(64))),
            now,
            file: Arc::new(Mutex::new(None)),
        }
    }

    /// Record one line, dropping the oldest once full and mirroring to the file.
    pub fn push(&self, text: impl Into<String>) {
        let line = LogLine {
            at: (self.now)(),
            text: text.into(),
        };
        if let Some(sink) = self.file.lock().unwrap().as_mut() {
            sink.write(&line.text);
        }
        let mut lines = self.lines.lock().unwrap();
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// Every retained line, oldest first.
    pub fn lines(&self) -> Vec<LogLine> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    /// The most recent `count` lines, oldest first — what a pane of that height
    /// can show.
    pub fn tail(&self, count: usize) -> Vec<LogLine> {
        let lines = self.lines.lock().unwrap();
        lines
            .iter()
            .skip(lines.len().saturating_sub(count))
            .cloned()
            .collect()
    }

    /// How many lines are retained.
    pub fn len(&self) -> usize {
        self.lines.lock().unwrap().len()
    }

    /// Whether nothing has been logged yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A [`LogFn`] the daemon runtime can be built with.
    pub fn sink(&self) -> LogFn {
        let buffer = self.clone();
        Arc::new(move |line: &str| buffer.push(line))
    }
}

#[cfg(test)]
mod tests {
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
}
