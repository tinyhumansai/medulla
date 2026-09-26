//! Plain-text DM handling: a raw message through the default provider.

use super::super::providers::{Abort, RunTaskOptions};
use super::super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Run a plain-text DM through the default provider, replying with raw text.
    pub(in crate::daemon) async fn handle_plain_text(&self, from: String, text: String) {
        let provider = self.inner.config.default_provider;
        if !self.inner.config.providers.contains(&provider) {
            self.send_raw(&from, "No coding agent is available on this daemon.")
                .await;
            return;
        }
        let Some(mut admission) = self.admit() else {
            // Prose for a human reading a DM, not the machine-readable rejection
            // the frame path sends: nothing parses this one.
            self.send_raw(
                &from,
                &format!(
                    "Daemon at capacity ({} pending tasks); retry later.",
                    self.inner.config.max_pending
                ),
            )
            .await;
            return;
        };
        let abort = Abort::new();
        admission.attach_controller(self.register_controller(abort.clone()));

        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        self.log(&format!("plaintext DM → {}", provider.as_str()));
        let options = RunTaskOptions {
            // A plain DM comes from the requester's own session, not from a
            // saved workflow graph, so it is a conversational run.
            origin: crate::daemon::providers::RunTaskOrigin::Conversation,
            conversation: from.clone(),
            // A conversational message continues the sender's session — that is
            // what makes a DM a conversation rather than a series of unrelated
            // one-shots.
            session_class: crate::sessions::SessionClass::Unbound,
            resume_session_id: None,
            workspace_context: Default::default(),
            provider,
            // A plain DM states no flavor; it runs the host's default transport.
            transport: crate::protocol::HarnessTransport::Cli,
            prompt: text,
            cwd: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            timeout_ms: self.inner.config.task_timeout_ms,
            model: self.inner.config.model.clone(),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort,
            router: self.inner.config.router.clone(),
            attribution: self.inner.config.attribution,
            hooks: self.inner.config.hooks.clone(),
            on_event: None,
            on_stdin: None,
            on_session: None,
            on_workspace_context: None,
        };
        let result = (self.inner.run_task)(options).await;
        match result {
            Ok(run) => self.send_raw(&from, &run.reply).await,
            Err(message) => {
                self.send_raw(&from, &format!("Task failed: {message}"))
                    .await
            }
        }
        drop(permit);
        drop(admission);
    }
}
