//! Translate a [`Cmd`] into a spawned async task whose result is reported back
//! over the [`AppMsg`] channel. Memory queries touch SQLite so they run on
//! `spawn_blocking` off the UI thread.
//!
//! Extracted from the main [`super::event_loop`] module so it stays under the
//! repository's 500-line ceiling.

use std::sync::Arc;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::Cmd;

use super::AppMsg;

#[cfg(feature = "workflows")]
mod copilot_hosts;
mod tasks;
#[cfg(feature = "workflows")]
mod workflows;

/// Move a copilot conversation onto the id of the workflow it just created.
///
/// Exposed here rather than reached into directly so the event loop has one
/// door into the host cache, matching how it reaches every other spawned
/// concern in this module.
#[cfg(feature = "workflows")]
pub(super) fn adopt_copilot_host(thread: &str, created: &str) {
    copilot_hosts::rename(thread, created);
}

/// End a copilot conversation whose workflow no longer exists.
#[cfg(feature = "workflows")]
pub(super) fn close_copilot_host(thread: &str) {
    copilot_hosts::forget(thread);
}

/// Drop every cached copilot host and stop its daemon.
///
/// The cache is process-global and keyed by workflow id, not by account — see
/// [`copilot_hosts::clear_all`] for why that makes a relogin the one place
/// this must be called.
#[cfg(feature = "workflows")]
pub(crate) fn clear_copilot_hosts() {
    copilot_hosts::clear_all();
}

/// No-op build without the `workflows` feature: there is no host cache to
/// clear, but the relogin call site is unconditional.
#[cfg(not(feature = "workflows"))]
pub(crate) fn clear_copilot_hosts() {}

/// Translate a [`Cmd`] emitted by the app into a spawned async task whose result
/// is reported back over the [`AppMsg`] channel. Memory queries touch SQLite so
/// they run on `spawn_blocking` off the UI thread.
///
/// `workflows_config` is the app's already-loaded `[workflows]` section,
/// threaded in for the two commands that dispatch a harness off-thread
/// ([`Cmd::RunWorkflow`] and [`Cmd::CopilotTurn`]) so they use the config the
/// TUI actually started with — including an explicit `--config` — rather than
/// rediscovering defaults from a fresh load. Named with a leading underscore
/// because a build without the `workflows` feature has no arm that reads it.
pub(super) fn run_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    _workflows_config: &medulla::config::WorkflowsConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let cmd = match tasks::run_task_cmd(cmd, msg_tx) {
        Some(cmd) => *cmd,
        None => return,
    };
    match cmd {
        Cmd::Quit => {}
        Cmd::LoadTasks
        | Cmd::SaveTask(_)
        | Cmd::SaveTasks(_)
        | Cmd::DeleteTask(_)
        | Cmd::SyncTasks(_) => unreachable!("task commands return before main dispatch"),
        Cmd::Submit(input) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            // What a resolved submit means is the runtime's to say: a blocking
            // wire has finished the cycle, a non-blocking one has only accepted
            // the message and is still producing.
            let settles = rt.submit_settles_cycle();
            tokio::spawn(async move {
                let status = match rt.submit(input).await {
                    Ok(()) if settles => "Cycle complete".to_string(),
                    Ok(()) => "Sent — waiting for the reply".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(AppMsg::Status(status));
            });
        }
        Cmd::Logout => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                // Only a clear that actually landed ends the session: reporting
                // success early would drop the operator back at the login screen
                // still signed in.
                let msg = match rt.logout().await {
                    Ok(()) => AppMsg::LoggedOut,
                    Err(e) => AppMsg::Status(format!("Account · logout failed: {e}")),
                };
                let _ = tx.send(msg);
            });
        }
        Cmd::Resume(id) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.resume_chat(id).await {
                    Ok(()) => {
                        let _ = tx.send(AppMsg::Resumed("Resumed chat".into()));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::ListChats => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.list_main_chats().await {
                    Ok(chats) => {
                        let _ = tx.send(AppMsg::OpenResume(chats));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::InspectContext => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.inspect_context().await {
                    Ok(items) => {
                        let _ = tx.send(AppMsg::Contexts(items));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::WatchTask { stop, start } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                // Stop first: a worker streaming a task nobody is looking at
                // spends a sample, a ratchet advance and a send every tick.
                if let Some((worker, task_id)) = stop {
                    let _ = rt.watch_task(worker, task_id, false).await;
                }
                if let Some((worker, task_id)) = start {
                    if let Err(e) = rt.watch_task(worker, task_id.clone(), true).await {
                        // Worth saying: the pane would otherwise just stay
                        // empty, which reads as a worker doing nothing.
                        let _ = tx.send(AppMsg::Status(format!("Cannot watch {task_id}: {e}")));
                    }
                }
            });
        }
        Cmd::WorkerOp(op) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let status = match rt.worker_op(op).await {
                    Ok(()) => "Worker registry updated".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(AppMsg::Status(status));
            });
        }
        #[cfg(feature = "workflows")]
        Cmd::RunWorkflow { id } => workflows::spawn_run(id, _workflows_config.clone(), msg_tx),
        #[cfg(feature = "workflows")]
        Cmd::DryRunWorkflow { id } => workflows::spawn_dry_run(id, msg_tx),
        #[cfg(feature = "workflows")]
        Cmd::UndoWorkflow { id } => workflows::spawn_undo(id, msg_tx),
        #[cfg(feature = "workflows")]
        Cmd::AbortCopilot { thread } => {
            // Inline: signalling an abort is a lock and a notify, and spawning
            // a task to do it would only delay the one thing the operator is
            // waiting for.
            let status = if copilot_hosts::abort(&thread) {
                "Stopping the copilot…"
            } else {
                "Nothing is running on this thread"
            };
            let _ = msg_tx.send(AppMsg::Status(status.to_string()));
        }
        #[cfg(feature = "workflows")]
        Cmd::RepairWorkflow {
            workflow,
            instruction,
            run_id,
        } => workflows::spawn_repair(
            workflow,
            instruction,
            run_id,
            _workflows_config.clone(),
            msg_tx,
        ),
        #[cfg(feature = "workflows")]
        Cmd::CopilotTurn {
            workflow,
            instruction,
        } => workflows::spawn_copilot(workflow, instruction, _workflows_config.clone(), msg_tx),
        #[cfg(feature = "workflows")]
        Cmd::CreateWorkflow {
            thread,
            instruction,
        } => {
            workflows::spawn_copilot_create(thread, instruction, _workflows_config.clone(), msg_tx)
        }
        Cmd::RefreshFleet => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                // A failed refresh keeps the previous fleet on screen; the
                // status line is where the failure belongs, not the tree.
                if let Err(e) = rt.refresh_fleet().await {
                    let _ = tx.send(AppMsg::Status(format!("fleet refresh failed: {e}")));
                }
            });
        }
        Cmd::LoadUsage => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.team_usage().await {
                    Ok(data) => {
                        let _ = tx.send(AppMsg::UsageLoaded(data));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(format!("usage fetch failed: {e}")));
                    }
                }
            });
        }
    }
}
