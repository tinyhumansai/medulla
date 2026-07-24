//! Translate a [`Cmd`] into a spawned async task whose result is reported back
//! over the [`AppMsg`] channel. Memory queries touch SQLite so they run on
//! `spawn_blocking` off the UI thread.
//!
//! Extracted from the main [`super::event_loop`] module so it stays under the
//! repository's 500-line ceiling.

use std::sync::Arc;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::Cmd;

use super::repo;
use super::AppMsg;

/// Translate a [`Cmd`] emitted by the app into a spawned async task whose result
/// is reported back over the [`AppMsg`] channel. Memory queries touch SQLite so
/// they run on `spawn_blocking` off the UI thread.
pub(super) fn run_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    memory: Option<Arc<medulla::memory::MemoryService>>,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    match cmd {
        Cmd::Quit => {}
        Cmd::Submit(input) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let status = match rt.submit(input).await {
                    Ok(()) => "Cycle complete".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(AppMsg::Status(status));
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
        Cmd::LoadWorkspaces(roots) => {
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let reports = roots
                    .into_iter()
                    .map(|root| {
                        let result = medulla::workspace::inspect_workspace(&root);
                        medulla::workspace::WorkspaceReport::from_result(root, result)
                    })
                    .collect();
                let _ = tx.send(AppMsg::WorkspacesLoaded(reports));
            });
        }
        Cmd::LoadWorkspaceDiff { workspace, path } => {
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let result =
                    medulla::workspace::diff(&workspace, &path).map_err(|error| error.to_string());
                let _ = tx.send(AppMsg::WorkspaceDiffLoaded {
                    workspace,
                    path,
                    result,
                });
            });
        }
        Cmd::PrepareReview {
            task_id,
            implementer_id,
            reviewer_id,
            workspace,
            contract,
        } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let diff_workspace = workspace.clone();
                let evidence = tokio::task::spawn_blocking(move || {
                    let paths = medulla::workspace::diff_name_only(&diff_workspace)
                        .map_err(|error| error.to_string())?;
                    let mut sections = Vec::new();
                    for path in &paths {
                        let patch = medulla::workspace::diff(&diff_workspace, path)
                            .map_err(|error| error.to_string())?;
                        if !patch.is_empty() {
                            sections.push(patch);
                        }
                    }
                    Ok::<_, String>((paths, sections.join("\n")))
                })
                .await;
                let (paths, diff) = match evidence {
                    Ok(Ok(evidence)) => evidence,
                    Ok(Err(error)) => {
                        let _ = tx.send(AppMsg::Status(format!("Review · diff failed: {error}")));
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(AppMsg::Status(format!(
                            "Review · diff task failed: {error}"
                        )));
                        return;
                    }
                };
                let instruction =
                    medulla::autoreview::compose_instruction(&medulla::autoreview::ReviewRequest {
                        task_id,
                        implementer_id,
                        reviewer_id,
                        workspace,
                        touched_paths: paths,
                        contract,
                        diff,
                    });
                let status = match rt.submit(instruction).await {
                    Ok(()) => "Review · delegated to a fresh reviewer".to_string(),
                    Err(error) => format!("Review · delegation failed: {error}"),
                };
                let _ = tx.send(AppMsg::Status(status));
            });
        }
        // --- ship panel (PR/CI) -----------------------------------------
        Cmd::LoadShip(roots) => {
            repo::load_ship(roots, msg_tx.clone());
        }
        Cmd::LoadShipLog { workspace, number } => {
            repo::load_ship_log(workspace, number, msg_tx.clone());
        }
        Cmd::OpenShipPr { workspace, number } => {
            repo::open_ship_pr(workspace, number, msg_tx.clone());
        }
        Cmd::CreateShipPr(workspace) => {
            repo::create_ship_pr(workspace, msg_tx.clone());
        }
        // --- feedback board ---------------------------------------------
        Cmd::LoadFeedback(query) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.list_feedback(query).await {
                    Ok(page) => {
                        let _ = tx.send(AppMsg::FeedbackLoaded(page));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::LoadFeedbackDetail(id) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.feedback_detail(id.clone()).await {
                    Ok(detail) => {
                        let _ = tx.send(AppMsg::FeedbackComments {
                            id,
                            comments: detail.comments,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::VoteFeedback { id, value } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.vote_feedback(id, value).await {
                    Ok(item) => {
                        let _ = tx.send(AppMsg::FeedbackItemUpdated(item));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::CommentFeedback { id, body } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let msg = match rt.comment_feedback(id, body).await {
                    Ok(_) => AppMsg::FeedbackChanged("Feedback · comment posted".into()),
                    Err(e) => AppMsg::Status(e.to_string()),
                };
                let _ = tx.send(msg);
            });
        }
        Cmd::SubmitFeedback { kind, title, body } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let msg = match rt.submit_feedback(kind, title, body).await {
                    // A moderation rejection is a successful call, not an error,
                    // so it must be surfaced explicitly — otherwise the
                    // submission looks like it silently vanished.
                    Ok(result) if result.accepted => {
                        AppMsg::FeedbackChanged("Feedback · submitted, thank you!".into())
                    }
                    Ok(result) => AppMsg::Status(format!(
                        "Feedback not published: {}",
                        if result.reason.is_empty() {
                            "rejected by moderation".into()
                        } else {
                            result.reason
                        }
                    )),
                    Err(e) => AppMsg::Status(e.to_string()),
                };
                let _ = tx.send(msg);
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
