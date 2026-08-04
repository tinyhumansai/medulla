//! Moving a harness between the operator and the orchestrator, and telling the
//! backend it moved.
//!
//! Both directions go through the roster and end in a re-advertisement. That is
//! the whole transport: `medulla:register_agents` is already re-emitted on every
//! roster mutation, so a control change is already an event — it only needed to
//! carry two more keys. A plane of its own would have meant a new subscriber on
//! the backend to say something the existing frame could already say.
//!
//! The caller names a *workspace*, not a worker. The TUI knows which directory a
//! harness is running in; which roster entry that maps to is the roster's own
//! business, and threading a worker id through the UI would have made every
//! caller re-derive it.

use super::super::{HandoffControl, HarnessHandoff};
use super::HubHandle;

impl HubHandle {
    /// Record that the operator has taken the harness covering `workspace`.
    ///
    /// Clears any handoff brief still hanging off that worker: a brief is an
    /// invitation to continue work in that workspace, and one on a harness the
    /// operator has just re-taken is an invitation the orchestrator cannot
    /// accept. Left there it would have the orchestrator plan a pass into a
    /// workspace it is refused from.
    ///
    /// Errors when no worker covers `workspace`. A host can be removed between
    /// the keystroke and this call, and reporting a successful takeover of a
    /// worker that is gone would leave the UI claiming a state that never was.
    pub async fn hold_harness(
        &self,
        workspace: &str,
        reason: Option<String>,
        at: i64,
    ) -> anyhow::Result<()> {
        let id = self.worker_for_workspace(workspace)?;
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

    /// Hand a harness back to the orchestrator, carrying the operator's brief.
    ///
    /// The brief is *context*, not permission. Control is already the
    /// orchestrator's by the time this lands — the local flag flips on the same
    /// keystroke, so the operator gets an immediate answer — and this is what
    /// tells the backend what the person was in the middle of. A handback whose
    /// brief fails to send is still a handback; the orchestrator simply picks
    /// the harness up without knowing the story, which is the pre-existing
    /// behaviour rather than a new failure.
    pub async fn hand_off_harness(&self, brief: HarnessHandoff) -> anyhow::Result<()> {
        let id = self.worker_for_workspace(&brief.workspace_path)?;
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

    /// The roster worker covering `workspace`.
    ///
    /// Exact match first, then the longest declared workspace that contains the
    /// path — a harness is often started in a subdirectory of the host's
    /// workspace, and the host is still the thing that owns it. Longest rather
    /// than first so nested hosts resolve to the nearer one.
    ///
    /// Deliberately no fall-back to "the selected worker". Attaching a brief to
    /// whichever host happened to be selected would advertise work as available
    /// in a workspace that has nothing to do with it, and the orchestrator would
    /// dispatch somewhere the operator never was.
    ///
    /// Resolves to **one** agent even when several are declared in that
    /// directory (the first exact match). Control is per-agent here and per
    /// *session* in the target model, so making a hold cover every agent in a
    /// checkout is part of the control work, not of declaring them.
    fn worker_for_workspace(&self, workspace: &str) -> anyhow::Result<String> {
        let path = workspace.trim_end_matches('/');
        let r = self.roster.lock().expect("roster lock");
        let mut best: Option<(usize, &str)> = None;
        for w in r.iter() {
            let Some(declared) = w.workspace_path().map(|d| d.trim_end_matches('/')) else {
                continue;
            };
            if declared == path {
                return Ok(w.id.clone());
            }
            let contains = path
                .strip_prefix(declared)
                .is_some_and(|rest| rest.starts_with('/'));
            if contains && best.is_none_or(|(len, _)| declared.len() > len) {
                best = Some((declared.len(), &w.id));
            }
        }
        best.map(|(_, id)| id.to_string())
            .ok_or_else(|| anyhow::anyhow!("no host declares a workspace covering {workspace}"))
    }
}
