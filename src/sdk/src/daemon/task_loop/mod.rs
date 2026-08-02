//! The frame- and task-handling half of [`DaemonRuntime`], split by what each
//! kind of frame asks for so no file exceeds the repo's 500-line ceiling:
//! [`probe`] answers the cached capability probe, [`system_info`] reports cheap
//! host capacity, [`control`] delivers mid-run input and stops a task the
//! requester has given up on, and [`run`] executes a task with its slot limit,
//! throttled status forwarding, and plain-text fallback.
//!
//! Routing and provider selection stay here: they are the seam the three share.
//! Lifecycle/dispatch/reply glue lives in [`super::runtime`].

mod control;
mod probe;
mod run;
mod system_info;
#[cfg(feature = "workflows")]
pub(in crate::daemon) mod workflow;

use crate::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind};

use super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Route a decoded task frame to its handler; responses are ignored.
    pub(super) async fn handle_frame(&self, from: String, frame: TaskFrame) {
        match frame.kind {
            // A frame naming a workflow runs that saved graph instead of
            // handing its text to a harness as an instruction.
            #[cfg(feature = "workflows")]
            TaskFrameKind::Task if frame.workflow.is_some() => {
                let id = frame.workflow.clone().unwrap_or_default();
                self.handle_workflow_task(from, frame, id).await
            }
            // A build without the engine must refuse a workflow frame rather
            // than fall through and hand its `text` to a harness: the text is a
            // trigger payload, not an instruction, and running it would do
            // something nobody asked for.
            #[cfg(not(feature = "workflows"))]
            TaskFrameKind::Task if frame.workflow.is_some() => {
                let id = frame.workflow.clone().unwrap_or_default();
                self.reply(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &format!(
                        "this worker was built without workflow support and cannot run '{id}'"
                    ),
                    frame.correlation_id.as_deref(),
                    None,
                )
                .await
            }
            TaskFrameKind::Task => self.handle_task(from, frame).await,
            TaskFrameKind::Input => self.handle_input(from, frame).await,
            TaskFrameKind::Abort => self.handle_abort(from, frame).await,
            TaskFrameKind::Capabilities => self.handle_capabilities(from, frame).await,
            TaskFrameKind::SystemInfo => self.handle_system_info(from, frame).await,
            // status/reply/error/ack/*_result are responses; ignore.
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

/// Add a task's workflow-tool mode and fleet depth to its harness environment.
///
/// Always written, never inherited: one daemon serves tasks at several depths,
/// so a value left over from its own environment could only ever be right for
/// one of them — and being wrong here means a fan-out guard that does not bite.
pub(super) fn with_tool_mode_at_depth(
    mut env: std::collections::HashMap<String, String>,
    mode: Option<&str>,
    depth: u8,
) -> std::collections::HashMap<String, String> {
    // These are capabilities for the MCP subprocess that received them, not
    // ambient configuration. A nested harness must obtain its own grant at its
    // actual depth; inheriting its parent's pair would bypass both depth and
    // session isolation, including on the direct-provider transport.
    env.remove(crate::control_socket::MCP_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_GRANT_ENV);
    env.insert(
        crate::control_socket::FLEET_DEPTH_ENV.to_string(),
        depth.to_string(),
    );
    #[cfg(feature = "workflows")]
    {
        env.remove(crate::mcp::TOOL_MODE_ENV);
        if let Some(mode) = mode {
            env.insert(crate::mcp::TOOL_MODE_ENV.to_string(), mode.to_string());
        }
        if mode.is_some() || task_can_reach_fleet(crate::control_socket::active().is_some()) {
            // Medulla tools are attached through ACP; legacy provider JSONL
            // cannot expose the MCP server. Restricted workflow turns need it
            // for authoring. Every task on the process that owns the control
            // plane needs ACP, including the root orchestrator at depth zero;
            // a remote worker cannot mint a fleet grant, so forcing ACP there
            // changes transport without adding any tools and can break an
            // otherwise installed direct CLI.
            env.insert(
                crate::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
                "acp".to_string(),
            );
        }
    }
    // Without the feature there is no MCP server to restrict, so the mode has
    // nowhere to go and nothing to protect.
    #[cfg(not(feature = "workflows"))]
    let _ = mode;
    env
}

/// Whether a full-mode task can receive fleet tools from this host.
#[cfg(feature = "workflows")]
pub(super) fn task_can_reach_fleet(has_active_plane: bool) -> bool {
    has_active_plane
}
