//! Reading and writing one thread's transcript.

use std::path::{Path, PathBuf};

use tinyinference::message::Message;

use super::types::Stored;
use super::MAX_REPLAYED;

/// The directory holding one account's thread transcripts.
fn root(home: &Path) -> PathBuf {
    home.join("agent-threads")
}

/// The file backing one thread.
///
/// The id reaches this from a workflow node, so it is not assumed to be a safe
/// filename: everything outside a conservative allowlist becomes `_`. That can
/// collide two ids onto one file, which is why the id is also recorded *inside*
/// the file and checked on read — a collision then reads as "no history" rather
/// than as another conversation's transcript.
pub(super) fn location(home: &Path, thread_id: &str) -> PathBuf {
    let safe: String = thread_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(96)
        .collect();
    root(home).join(format!("{safe}.json"))
}

/// The messages to replay for `thread_id`, oldest first.
///
/// Empty for a thread with no history — a fresh node, a first turn, or a
/// transcript that cannot be read. Unreadable is deliberately not an error: the
/// turn can still run, and refusing to answer because a cache is corrupt is a
/// worse outcome than answering without the context.
pub fn load(home: &Path, thread_id: &str) -> Vec<Message> {
    let path = location(home, thread_id);
    let Ok(body) = std::fs::read(&path) else {
        return Vec::new();
    };
    let stored: Stored = match serde_json::from_slice(&body) {
        Ok(stored) => stored,
        Err(err) => {
            tracing::debug!("[agent] ignoring unreadable transcript at {path:?}: {err}");
            return Vec::new();
        }
    };
    if stored.thread_id != thread_id {
        tracing::debug!("[agent] transcript at {path:?} belongs to another thread; ignoring");
        return Vec::new();
    }
    super::trim(stored.messages, MAX_REPLAYED)
}

/// Serializes writers per thread within this process.
///
/// The atomic replacement in [`crate::auth::write_private_file`] stops a
/// *partial* file, and does nothing about a lost update: two turns resuming the
/// same thread each load the same prior transcript, each build a complete
/// replacement, and whichever writes last erases the other's turn from history.
///
/// A lock per thread id rather than one global lock, so two unrelated
/// conversations do not serialize against each other. Entries are never removed
/// — a thread id is bounded by the conversations this process actually ran, and
/// a mutex per one of those is cheaper than the bookkeeping to reclaim it.
///
/// This is a within-process guarantee only. Two Medulla processes resuming one
/// thread would still race, which needs a file lock rather than a mutex; no
/// caller does that today, and pretending otherwise would be worse than saying
/// so.
static THREAD_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<()>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// The lock guarding one thread's transcript.
fn thread_lock(thread_id: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    let mut locks = THREAD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks
        .entry(thread_id.to_string())
        .or_insert_with(|| std::sync::Arc::new(std::sync::Mutex::new(())))
        .clone()
}

/// Record `messages` as `thread_id`'s transcript, replacing any previous one.
///
/// Best-effort: a failure to persist is logged and swallowed. The turn has
/// already happened and its answer is already on its way to the caller, so
/// failing here would discard real work over a cache write.
///
/// Serialized per thread: two turns resuming one conversation cannot lose
/// each other's messages to a last-writer-wins replacement.
pub fn save(home: &Path, thread_id: &str, messages: &[Message]) {
    let lock = thread_lock(thread_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    save_locked(home, thread_id, messages)
}

/// The write itself, with the thread's lock already held.
fn save_locked(home: &Path, thread_id: &str, messages: &[Message]) {
    // Trimmed on the way in as well as the way out, so a long-running thread's
    // file does not grow without bound just because reads cap it — and so the
    // stored sequence is one a provider would accept as-is.
    let stored = Stored {
        thread_id: thread_id.to_string(),
        messages: super::trim(messages.to_vec(), MAX_REPLAYED),
    };

    let path = location(home, thread_id);
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = create_private_dir(parent) {
        tracing::debug!("[agent] could not create {parent:?}: {err}");
        return;
    }
    match serde_json::to_vec(&stored) {
        Ok(body) => {
            // Owner-only, like the session store beside it. A transcript holds
            // the operator's prompts, file contents the turn read, and tool
            // output — on a shared host that is workspace data other local users
            // have no business reading, even though it is not a bearer token.
            if let Err(err) = crate::auth::write_private_file(&path, &body) {
                tracing::debug!("[agent] could not write transcript {path:?}: {err}");
            }
        }
        Err(err) => tracing::debug!("[agent] could not encode transcript: {err}"),
    }
}

/// Create the transcript directory, owner-only on unix.
///
/// The default `create_dir_all` mode is `0777 & !umask`, which on the usual
/// `022` is world-readable — and a directory listing alone discloses which
/// threads exist. Set explicitly rather than left to the umask.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
