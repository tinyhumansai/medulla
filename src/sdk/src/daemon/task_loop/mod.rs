//! The frame- and task-handling half of [`DaemonRuntime`], split by what each
//! kind of frame asks for so no file exceeds the repo's 500-line ceiling:
//! [`probe`] answers the cached capability probe, [`control`] delivers mid-run
//! input and stops a task the requester has given up on, and [`run`] executes a
//! task with its slot limit, throttled status forwarding, and plain-text
//! fallback.
//!
//! Routing and provider selection stay here: they are the seam the three share.
//! Lifecycle/dispatch/reply glue lives in [`super::runtime`].

mod control;
mod probe;
mod run;

use crate::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind};

use super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Route a decoded task frame to its handler; responses are ignored.
    pub(super) async fn handle_frame(&self, from: String, frame: TaskFrame) {
        match frame.kind {
            TaskFrameKind::Task => self.handle_task(from, frame).await,
            TaskFrameKind::Input => self.handle_input(from, frame).await,
            TaskFrameKind::Abort => self.handle_abort(from, frame).await,
            TaskFrameKind::Capabilities => self.handle_capabilities(from, frame).await,
            // status/reply/error/ack/capabilities_result are responses; ignore.
            _ => {}
        }
    }

    /// Choose a provider: the requested one if offered, else the default, else
    /// the first offered.
    fn select_provider(&self, requested: Option<HarnessProvider>) -> Option<HarnessProvider> {
        let providers = &self.inner.config.providers;
        match requested {
            Some(requested) => providers.contains(&requested).then_some(requested),
            None => {
                if providers.contains(&self.inner.config.default_provider) {
                    Some(self.inner.config.default_provider)
                } else {
                    providers.first().copied()
                }
            }
        }
    }
}
