//! Data types for the `session` module.
#[allow(unused_imports)]
use super::*;
/// How a session's child process should be started.
#[derive(Debug, Clone)]
pub struct InteractiveSpec {
    /// Which CLI to spawn. Only [`HarnessProvider::Claude`] supports this
    /// transport today; see
    /// [`can_run_interactive`](crate::sessions::can_run_interactive).
    pub provider: HarnessProvider,
    /// The resolved binary name or path.
    pub bin: String,
    /// Working directory for the child.
    pub cwd: String,
    /// The full environment the child runs with (the parent env is cleared).
    pub env: std::collections::HashMap<String, String>,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional system-prompt suffix.
    pub append_system_prompt: Option<String>,
    /// Whether to pass the provider's skip-permissions flag.
    pub skip_permissions: bool,
    /// Extra argv appended to the built base args.
    pub extra_args: Vec<String>,
    /// Lifecycle hooks applied through the provider-specific launch adapter.
    pub hooks: crate::harness_hooks::HooksConfig,
}
/// A live interactive harness process.
pub struct InteractiveSession {
    pub(super) child: Mutex<Option<Child>>,
    pub(super) stdin: AsyncMutex<Option<ChildStdin>>,
    /// Semantic events from the reader task. Held behind a mutex so exactly one
    /// turn consumes the stream at a time.
    pub(super) events: AsyncMutex<mpsc::UnboundedReceiver<StreamEvent>>,
    /// The harness's own session id, once announced. Observability only.
    pub(super) harness_session_id: Mutex<Option<String>>,
    pub(super) interrupt_seq: AtomicU64,
}
