//! Moving a harness between the operator and the orchestrator, and telling the
//! backend it moved.
//!
//! Both directions go through the roster and end in a re-advertisement. That is
//! the whole transport: `medulla:register_agents` is already re-emitted on every
//! roster mutation, so a control change is already an event — it only needed to
//! carry two more keys. A plane of its own would have meant a new subscriber on
//! the backend to say something the existing frame could already say.

use super::super::{HandoffControl, HarnessHandoff};
use super::HubHandle;

impl HubHandle {
    /// Record that the operator has taken `id`'s harness, and say so.
    ///
    /// Clears any handoff brief still hanging off the worker: a brief is an
    /// invitation to continue work in that workspace, and one on a harness the
    /// operator has just re-taken is an invitation the orchestrator cannot
    /// accept. Leaving it would have it plan a pass into a workspace it is
    /// refused from.
    ///
    /// Errors when no worker holds `id`. A host can be removed between the
    /// keystroke and this call, and reporting a successful takeover of a worker
    /// that is gone would leave the UI claiming a state that never existed.
    pub async fn hold_harness(
        &self,
        id: &str,
        reason: Option<String>,
        at: i64,
    ) -> anyhow::Result<()> {
        {
            let mut r = self.roster.lock().expect("roster lock");
            let Some(w) = r.iter_mut().find(|w| w.id == id) else {
                anyhow::bail!("no host {id} to take a harness from");
            };
            w.control = HandoffControl::Operator;
            w.control_reason = reason
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty());
            w.control_since = Some(at);
            w.handoff = None;
        }
        self.reregister().await
    }

    /// Hand `id`'s harness back to the orchestrator, with the operator's brief.
    ///
    /// The brief is *context*, not permission. Control is already the
    /// orchestrator's by the time this lands — the local flag flips first, so the
    /// operator gets an answer on the same keystroke — and this is the message
    /// that tells the backend what the person was in the middle of. A handback
    /// whose brief fails to send is still a handback; the orchestrator simply
    /// picks the harness up without knowing the story.
    pub async fn hand_off_harness(&self, id: &str, brief: HarnessHandoff) -> anyhow::Result<()> {
        {
            let mut r = self.roster.lock().expect("roster lock");
            let Some(w) = r.iter_mut().find(|w| w.id == id) else {
                anyhow::bail!("no host {id} to hand a harness back on");
            };
            w.control = HandoffControl::Orchestrator;
            w.control_reason = None;
            w.control_since = None;
            w.handoff = Some(brief);
        }
        self.reregister().await
    }
}
