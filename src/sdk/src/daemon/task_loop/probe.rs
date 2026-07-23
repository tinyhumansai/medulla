//! Answering a peer's capability probe from the cached snapshot.

use crate::tinyplace::{AgentCapabilities, TaskFrame, TaskFrameKind};

use super::super::capabilities::{probe_capabilities, ProbeOptions};
use super::super::providers::Abort;
use super::super::types::{DaemonRuntime, DEFAULT_CAPABILITY_TIMEOUT_MS};

impl DaemonRuntime {
    /// Answer a `capabilities` probe with the cached [`AgentCapabilities`].
    pub(super) async fn handle_capabilities(&self, from: String, frame: TaskFrame) {
        let capabilities = self.get_capabilities().await;
        let text = serde_json::to_string(&capabilities).unwrap_or_else(|_| "{}".to_string());
        self.reply(
            &from,
            TaskFrameKind::CapabilitiesResult,
            &frame.task_id,
            &text,
            frame.correlation_id.as_deref(),
            Some(self.inner.config.default_provider),
        )
        .await;
    }

    /// Return the capabilities, probing once and caching the result.
    async fn get_capabilities(&self) -> AgentCapabilities {
        // Single cached probe shared across askers: holding the async mutex across
        // the probe means concurrent callers wait for the one run, then serve from
        // cache. Deliberately NOT counted against maxPending.
        let mut guard = self.inner.capabilities.lock().await;
        if let Some(capabilities) = guard.as_ref() {
            return capabilities.clone();
        }
        let provider = self
            .select_provider(None)
            .unwrap_or(self.inner.config.default_provider);
        self.log(&format!("capability probe → {}", provider.as_str()));
        let abort = Abort::new();
        let controller_id = self.register_controller(abort.clone());
        // Compete for the concurrency budget like a task.
        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        let capabilities = probe_capabilities(ProbeOptions {
            provider,
            run_task: self.inner.run_task.clone(),
            workspace: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            providers: self.inner.config.providers.clone(),
            timeout_ms: self
                .inner
                .config
                .capability_timeout_ms
                .or(Some(DEFAULT_CAPABILITY_TIMEOUT_MS)),
            model: self.inner.config.model.clone(),
            agent: self.inner.config.agent.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort,
        })
        .await;
        drop(permit);
        self.unregister_controller(controller_id);
        *guard = Some(capabilities.clone());
        capabilities
    }
}
