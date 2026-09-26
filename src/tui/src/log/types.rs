//! Data types for the `log` module.
#[allow(unused_imports)]
use super::*;
/// A file every log line is appended to.
pub(super) struct FileSink {
    pub(super) path: PathBuf,
    pub(super) handle: Option<File>,
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
    pub(super) lines: Arc<Mutex<VecDeque<LogLine>>>,
    pub(super) now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub(super) file: Arc<Mutex<Option<FileSink>>>,
}
