//! Sending a handoff brief, off the UI thread.
//!
//! Two things here must not happen on the render thread: reading the git branch
//! shells out to a child process, and the emit itself awaits a socket. Both are
//! fast in the ordinary case and neither is bounded in the bad one.
//!
//! Every outcome is narrated. A brief that silently fails to send is precisely
//! the failure this whole feature exists to remove — the operator would be told
//! the harness was handed back, believe the orchestrator had their note, and
//! never find out otherwise.

use std::sync::Arc;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::Cmd;

use super::super::AppMsg;

/// Spawn a handoff command, returning anything else to the caller.
pub(super) fn run_handoff_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) -> Option<Box<Cmd>> {
    match cmd {
        Cmd::HandOffHarness(brief) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let mut brief = *brief;
                // Read here rather than in the UI: this is the one fact in the
                // brief that costs a subprocess. Best-effort — a workspace that
                // is not a git repo simply hands over without a branch, which is
                // still a useful brief.
                let facts =
                    medulla::daemon::capabilities::read_git_facts(&brief.workspace_path).await;
                brief.branch = facts.branch;
                brief.project = facts.project;

                let message = match rt.hand_off_harness(brief).await {
                    Ok(()) => "Handed back · the orchestrator has your brief".to_string(),
                    // Names both halves: the harness *was* handed back (that
                    // part is local and already done), and what was lost is the
                    // context. An operator who reads this knows to say it again
                    // in chat rather than assuming the work is queued.
                    Err(e) => format!(
                        "Handed back · your brief did not send: {e} \
                         (the orchestrator may pick it up without context)"
                    ),
                };
                let _ = tx.send(AppMsg::Status(message));
            });
            None
        }
        Cmd::HoldHarness { workspace, reason } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                // Only the failure is narrated. Taking a harness already reports
                // itself on the status line the moment the key is pressed, and
                // a second line saying the same thing would push it off.
                if let Err(e) = rt.hold_harness(workspace, reason).await {
                    let _ = tx.send(AppMsg::Status(format!(
                        "You have this harness · the orchestrator was not told: {e}"
                    )));
                }
            });
            None
        }
        other => Some(Box::new(other)),
    }
}
