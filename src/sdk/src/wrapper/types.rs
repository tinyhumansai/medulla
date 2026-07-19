//! Data model for a wrapped session: the caller-facing [`WrapperConfig`] and the
//! internal [`WrapperTimings`] resolved from the environment. These are pure data
//! plus their trivial constructors; the behaviour that consumes them lives in
//! [`run`](super::run) and [`bridge`](super::bridge).

use std::collections::HashMap;

use crate::tinyplace::HarnessProvider;

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
}
