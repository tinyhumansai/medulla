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
mod tests;

mod types;
use types::FileSink;
pub use types::LogBuffer;
pub use types::LogLine;
