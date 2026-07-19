//! Provider detection + headless one-shot task execution, ported from the
//! tinyplace CLI `daemon/providers.ts`.
//!
//! The daemon runs a delegated task by spawning the requested coding-agent CLI
//! once, non-interactively, and folding its streaming JSONL output through the
//! shared [`super::mappers`] semantic-event mappers to derive status updates and
//! the final agent message. This is the headless complement to the interactive
//! PTY wrapper (which lands separately).
//!
//! Split by responsibility: [`types`] holds the data model (callback aliases,
//! the [`Abort`] handle, and the run input/output records), [`detect`] the
//! provider discovery / binary resolution / argv building, and [`execute`] the
//! spawn-and-stream run loop with its transient-lock retry. All public items are
//! re-exported here so callers use `medulla::daemon::providers::*`.

mod detect;
mod execute;
mod types;

#[cfg(test)]
mod tests;

pub use detect::{
    build_run_args, detect_providers, make_path_lookup, provider_bin, provider_name,
    supports_stdin, DAEMON_PROVIDERS,
};
pub use execute::{is_transient_lock, run_provider_task, with_auth_hint};
pub use types::{Abort, ExistsOnPath, OnEvent, OnStdin, RunTaskFn, RunTaskOptions, RunTaskResult};
