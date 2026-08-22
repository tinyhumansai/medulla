//! The frame- and task-handling half of [`DaemonRuntime`], split by what each
//! kind of frame asks for so no file exceeds the repo's 500-line ceiling:
//! [`probe`] answers the cached capability probe, [`system_info`] reports cheap
//! host capacity, [`control`] delivers mid-run input and stops a task the
//! requester has given up on, [`register`] claims the per-task running record,
//! [`plain`] runs a text DM through the default provider, and [`run`] executes a
//! task frame with its slot limit and throttled status forwarding.
//!
//! Routing and provider selection stay here: they are the seam the three share.
//! Lifecycle/dispatch/reply glue lives in [`super::runtime`].

mod control;
mod plain;
mod probe;
mod register;
mod run;
mod system_info;
#[cfg(test)]
mod tests;
#[cfg(feature = "workflows")]
pub(in crate::daemon) mod workflow;

use crate::protocol::{HarnessProvider, TaskFrame, TaskFrameKind};

use super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Route a decoded task frame to its handler; responses are ignored.
    ///
    /// `sender_device_local` is the receiver's own verdict on `from`, carried in
    /// from the transport that delivered the frame (see
    /// [`handle_message_from`](crate::daemon::DaemonRuntime::handle_message_from)).
    /// It alone may let a plain task frame's `workflow_node` marker buy
    /// [`RunTaskOrigin::Workflow`](crate::daemon::providers::RunTaskOrigin::Workflow);
    /// [`run::handle_task`](Self::handle_task) is where that decision is gated.
    pub(super) async fn handle_frame(
        &self,
        from: String,
        frame: TaskFrame,
        sender_device_local: bool,
    ) {
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
            TaskFrameKind::Task => self.handle_task(from, frame, sender_device_local).await,
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
            Some(requested) => {
                // A task that named the embedded core gets it, whether or not
                // this worker "offers" it. `config.providers` is the list of
                // coding CLIs found on PATH (see
                // [`crate::daemon::providers::detect_providers`]), and
                // OpenHuman is never on it — it has no binary to find.
                // Rejecting it here would admit a direct `harness: "openhuman"`
                // task on the control socket only to refuse it, and
                // substituting a coding CLI would change what the task is for.
                // Every other request must be offered: an uninstalled CLI named
                // by a requester is a genuine "no available provider" failure.
                if requested == HarnessProvider::Openhuman {
                    return Some(requested);
                }
                providers.contains(&requested).then_some(requested)
            }
            None => {
                // An OpenHuman default is honoured the same way an explicit
                // request for it is: the embedded core needs no binary to
                // detect, so an operator who configured `openhuman` as the
                // default means exactly that, even on a machine where no coding
                // CLI was found.
                if self.inner.config.default_provider == HarnessProvider::Openhuman {
                    return Some(self.inner.config.default_provider);
                }
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
    let has_parent_grant = crate::control_socket::parent_grant_from_env(&env).is_some();
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
        if mode.is_some()
            || has_parent_grant
            || task_can_reach_fleet(crate::control_socket::active().is_some())
        {
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
