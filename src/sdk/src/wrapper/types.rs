//! Data model for a wrapped session: the caller-facing [`WrapperConfig`] and the
//! internal [`WrapperTimings`] resolved from the environment. These are pure data
//! plus their trivial constructors; the behaviour that consumes them lives in
//! [`run`](super::run) and [`bridge`](super::bridge).

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};

use crate::tinyplace::HarnessProvider;

/// What the app-side spawner needs in order to launch the harness on a PTY.
pub struct PtyRequest {
    /// The resolved coding-agent binary to execute.
    pub bin: String,
    /// The full child argv (provider args, attribution, then user args).
    pub args: Vec<String>,
    /// Working directory for the child.
    pub cwd: String,
    /// Environment overlay applied on top of the inherited environment.
    pub env: HashMap<String, String>,
}

/// A harness running on a pseudo-terminal, handed back by a [`PtySpawner`].
///
/// The wrapper drives this without knowing how the PTY was allocated, which is
/// what keeps the SDK free of any terminal dependency.
pub struct PtyHarness {
    /// Sink for bytes typed into the child: both the operator's own keystrokes
    /// (forwarded by the spawner) and messages injected from tiny.place.
    pub input: mpsc::UnboundedSender<Vec<u8>>,
    /// Resolves with the child's shell-style exit code once it exits.
    pub done: oneshot::Receiver<i32>,
    /// Sending on this kills the child.
    pub kill: oneshot::Sender<()>,
    /// Resolves once the child's final output has been copied to our stdout.
    /// The wrapper waits on this (briefly) before restoring the terminal.
    pub drained: oneshot::Receiver<()>,
    /// Returns the terminal to cooked mode. The wrapper calls this after the
    /// child's output has drained; dropping it without calling has the same
    /// effect, so a panic still restores the terminal.
    pub restore: Box<dyn FnOnce() + Send>,
}

/// App-side seam for running the harness on a real pseudo-terminal.
///
/// The SDK invokes this only when input injection is active *and* stdin is a
/// terminal — the one case where the child needs a tty of its own but we still
/// need a writable handle on its input. When it is `None`, or it returns an
/// error, the wrapper falls back to inherited stdio.
///
/// This mirrors [`OnboardingUi`](crate::onboarding::OnboardingUi): the terminal
/// implementation lives in the app crate, not here.
///
/// `Sync` is required because [`WrapperConfig`] is borrowed across awaits in the
/// run loop, which the spawned wrapper future then carries between threads.
pub type PtySpawner = Box<dyn FnOnce(PtyRequest) -> anyhow::Result<PtyHarness> + Send + Sync>;

/// Poll intervals and status timings for one wrapped session, resolved from the
/// environment (see [`crate::tinyplace::env`]).
pub(super) struct WrapperTimings {
    /// How often the transcript tailer polls for new lines, in milliseconds.
    pub(super) tail_poll_ms: u64,
    /// How often the inbox is drained for inbound owner input, in milliseconds.
    pub(super) receive_poll_ms: u64,
    /// Minimum spacing between status envelopes (heartbeat throttle), in ms.
    pub(super) status_throttle_ms: i64,
    /// Idle span after which the session is considered idle, in milliseconds.
    pub(super) status_idle_ms: i64,
}

impl WrapperTimings {
    /// Resolve all timings for `provider` from `env`, falling back to the
    /// per-provider defaults in [`crate::tinyplace::env`].
    pub(super) fn resolve(provider: HarnessProvider, env: &HashMap<String, String>) -> Self {
        use crate::tinyplace::env as tp_env;
        WrapperTimings {
            tail_poll_ms: tp_env::session_poll_ms(provider, env),
            receive_poll_ms: tp_env::receive_poll_ms(provider, env),
            status_throttle_ms: tp_env::status_heartbeat_ms(provider, env) as i64,
            status_idle_ms: tp_env::status_idle_ms(provider, env) as i64,
        }
    }
}

/// Everything a wrapper run needs. Built from the process environment by
/// [`run_wrapper`](super::run::run_wrapper); constructed explicitly by tests.
pub struct WrapperConfig {
    /// The coding-agent provider this session wraps.
    pub provider: HarnessProvider,
    /// Arguments passed through to the child CLI verbatim.
    pub child_args: Vec<String>,
    /// The environment used for config/session-dir/bin resolution and applied as
    /// an overlay on the child's inherited environment.
    pub env: HashMap<String, String>,
    /// The working directory the child runs in and the session is anchored to.
    pub cwd: String,
    /// Pure passthrough: never activate the tiny.place bridge.
    pub no_bridge: bool,
    /// Override the generated wrapper session id (deterministic tests).
    pub session_id: Option<String>,
    /// App-side PTY spawner. `None` (the default, and what tests use) keeps the
    /// child on inherited or piped stdio.
    pub pty_spawner: Option<PtySpawner>,
}
