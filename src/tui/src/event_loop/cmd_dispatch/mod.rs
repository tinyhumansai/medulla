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
    memory: Option<Arc<medulla::memory::MemoryService>>,
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
        // Memory queries are synchronous but touch SQLite, so run them off the UI
        // thread via `spawn_blocking` and report back over `AppMsg`.
        Cmd::LoadMemory => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let (status, directives) = read_memory(&rt, memory.as_deref());
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
            });
        }
        Cmd::SearchMemory(query) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let (status, directives) = read_memory(&rt, memory.as_deref());
                let hits = match memory.as_deref() {
                    Some(svc) => svc.search(&query, None, 20),
                    None => rt.memory_search(query.clone(), None, 20),
                };
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
                let _ = tx.send(AppMsg::MemoryResults { hits, query });
            });
        }
        // Ingest is genuinely long-running (it walks transcripts and calls a
        // paid summarizer), so it runs as a normal async task and reports a
        // single terminal status rather than streaming progress.
        Cmd::IngestMemory { backfill } => {
            let tx = msg_tx.clone();
            let Some(svc) = memory else {
                let _ = tx.send(AppMsg::MemoryIngestDone(
                    "Memory · no memory service is attached; nothing to ingest".into(),
                ));
                return;
            };
            let rt = runtime.clone();
            // `ingest` returns a non-`Send` future, so it cannot ride
            // `tokio::spawn`. Give it a dedicated blocking thread with its own
            // current-thread runtime — that also keeps a long walk off the
            // shared worker pool, where it would starve UI-facing tasks.
            tokio::task::spawn_blocking(move || {
                let mode = if backfill {
                    medulla::memory::IngestMode::Backfill
                } else {
                    medulla::memory::IngestMode::Incremental
                };
                let local = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(AppMsg::MemoryIngestDone(format!(
                            "Memory · ingest failed to start: {e}"
                        )));
                        return;
                    }
                };
                let status = match local.block_on(svc.ingest(mode)) {
                    // Mirrors `medulla memory`'s summary line. The budget note
                    // matters: a truncated run looks identical otherwise, and
                    // the user needs to know more work remains.
                    Ok(report) => format!(
                        "Memory · {} complete — {} files, {} sessions, {} observations{}",
                        report.mode,
                        report.files_seen,
                        report.sessions_processed,
                        report.observations,
                        if report.budget_hit {
                            " (budget hit — rerun to continue)"
                        } else {
                            ""
                        },
                    ),
                    Err(e) => format!("Memory · ingest failed: {e}"),
                };
                // Re-read so the tab reflects the store the ingest just grew.
                let (st, directives) = read_memory(&rt, Some(svc.as_ref()));
                let _ = tx.send(AppMsg::MemoryLoaded {
                    status: st,
                    directives,
                });
                let _ = tx.send(AppMsg::MemoryIngestDone(status));
            });
        }
    }
}

/// Read memory status + directives, preferring the directly-attached service
/// over the runtime seam.
///
/// The runtime seam only carries memory on the core runtime, so without this the
/// Memory tab would be empty on the backend and mock paths. The seam remains the
/// fallback because the mock scripts memory through it in tests.
pub(super) fn read_memory(
    runtime: &Arc<dyn Runtime>,
    memory: Option<&medulla::memory::MemoryService>,
) -> (Option<medulla::memory::MemoryStatus>, Vec<String>) {
    match memory {
        Some(svc) => (Some(svc.status()), svc.directives()),
        None => (runtime.memory_status(), runtime.memory_directives()),
    }
}
