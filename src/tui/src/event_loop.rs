//! The interactive TUI event loop and its async plumbing.
//!
//! Owns [`AppMsg`] (the messages spawned tasks send back to the UI), the main
//! [`run`] select-loop over crossterm events / runtime pings / a 90ms tick,
//! [`run_cmd`] which turns a [`Cmd`] into a spawned async task, and the
//! background [`spawn_update_checker`]. The loop keeps all mutation on one task
//! and folds async results back in over an mpsc channel.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::EventStream;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, Cmd, TABS};

use crate::terminal::set_mouse_capture;

mod types;

#[cfg(test)]
mod tests;

use types::AppMsg;
pub(crate) use types::{SessionExit, SessionWiring};

/// Drive the ratatui app: build [`App`], subscribe to the runtime, and loop over
/// input events, runtime snapshots, background [`AppMsg`]s, and the animation
/// tick until the app requests quit.
pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    runtime: Arc<dyn Runtime>,
    wiring: SessionWiring,
) -> anyhow::Result<SessionExit> {
    let SessionWiring {
        loaded,
        startup_status,
        tinyplace_obs,
        config_path,
        medulla_home,
        memory_service,
        mut sharing,
        onboarding_path,
    } = wiring;
    let mut app = App::new(runtime.clone(), loaded);
    app.set_config_path(config_path);
    app.set_medulla_home(medulla_home);
    if let Some(svc) = memory_service {
        app.set_memory_service(svc);
    }
    if let Some(obs) = tinyplace_obs {
        app.set_tinyplace_observation(obs);
    }
    if let Some(status) = startup_status {
        app.set_status(status);
    }
    let mut sub = runtime.subscribe();
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AppMsg>();
    let mut mouse_on = true;

    // Background release-update checker ("automated cron"): first probe ~10s
    // after startup, then every 6h. A newer version surfaces as a persistent
    // header banner. Disabled via `[update] check = false` or
    // `MEDULLA_NO_UPDATE_CHECK`.
    spawn_update_checker(&app.loaded, &msg_tx);

    loop {
        terminal.draw(|f| app.draw(f))?;
        if app.should_quit {
            break;
        }
        if app.mouse_capture != mouse_on {
            mouse_on = app.mouse_capture;
            set_mouse_capture(mouse_on);
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(ev)) = maybe_event {
                    if let Some(cmd) = app.on_event(ev) {
                        run_cmd(cmd, &runtime, app.memory_service(), &msg_tx);
                    }
                }
            }
            recv = sub.recv() => {
                if recv.is_ok() {
                    app.refresh_snapshot();
                    if app.tab() == "Context" && app.events_changed() {
                        run_cmd(Cmd::InspectContext, &runtime, app.memory_service(), &msg_tx);
                    }
                }
            }
            Some(msg) = msg_rx.recv() => {
                match msg {
                    AppMsg::Status(s) => { app.set_status(s); app.refresh_snapshot(); }
                    AppMsg::Contexts(c) => app.set_contexts(c),
                    AppMsg::UsageLoaded(data) => app.set_account_usage(data),
                    AppMsg::OpenResume(chats) => app.open_resume(chats),
                    AppMsg::Resumed(s) => {
                        app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap_or(1);
                        app.refresh_snapshot();
                        app.set_status(s);
                    }
                    AppMsg::MemoryLoaded { status, directives } => {
                        app.set_memory_loaded(status, directives);
                    }
                    AppMsg::MemoryIngestDone(status) => {
                        app.set_memory_ingest_done(status);
                    }
                    AppMsg::MemoryResults { hits, query } => {
                        let n = hits.len();
                        app.set_memory_results(hits, query);
                        app.set_status(format!("Memory · {n} hit(s)"));
                    }
                    AppMsg::FeedbackLoaded(page) => {
                        app.set_feedback_page(page);
                        // Pull the newly selected row's comments in the same beat.
                        if let Some(cmd) = app.feedback_detail_cmd() {
                            run_cmd(cmd, &runtime, app.memory_service(), &msg_tx);
                        }
                    }
                    AppMsg::FeedbackComments { id, comments } => {
                        app.set_feedback_comments(id, comments);
                    }
                    AppMsg::FeedbackItemUpdated(item) => {
                        app.apply_feedback_item(item);
                        app.set_status("Feedback · vote recorded");
                    }
                    AppMsg::FeedbackChanged(status) => {
                        app.set_status(status);
                        // A comment or submission changes the board, so re-pull
                        // it rather than patching state locally.
                        run_cmd(Cmd::LoadFeedback(app.feedback_query()), &runtime, app.memory_service(), &msg_tx);
                    }
                    AppMsg::UpdateAvailable(notice) => {
                        app.set_update_notice(notice.clone());
                        app.set_status(notice);
                        app.refresh_snapshot();
                    }
                }
            }
            // A history share the welcome flow handed over. Reported on the
            // status line so the user sees it land without ever being blocked
            // by it. `recv` on a `None` receiver would be `Poll::Pending`
            // forever, so the arm is disabled outright when nothing is running.
            Some(ev) = async { sharing.as_mut()?.recv().await }, if sharing.is_some() => {
                let status = medulla_tui::ui::welcome::share_status(&ev, || {
                    medulla_tui::ui::welcome::persist_onboarding(&onboarding_path)
                });
                if let Some(status) = status {
                    app.set_status(status);
                }
                // A settled share is the last thing this channel will say. Drop
                // it so the arm stops being polled.
                if medulla_tui::ui::welcome::settles_share(&ev) {
                    sharing = None;
                }
            }
            _ = tick.tick() => {
                if app.snapshot.running {
                    app.frame = app.frame.wrapping_add(1);
                }
            }
        }
    }
    Ok(if app.relogin_requested() {
        SessionExit::Relogin
    } else {
        SessionExit::Quit
    })
}

/// Spawn the periodic release-update checker unless disabled by config/env. It
/// waits ~10s, checks once, then rechecks every 6h, sending [`AppMsg::UpdateAvailable`]
/// on a newer release.
fn spawn_update_checker(
    loaded: &medulla::config::LoadedConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    if !loaded.config.update.enabled(&env) {
        return;
    }
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let url = medulla::update::update_url();
        let current = env!("CARGO_PKG_VERSION");
        let mut first = true;
        loop {
            let delay = if first {
                Duration::from_secs(10)
            } else {
                Duration::from_secs(6 * 60 * 60)
            };
            first = false;
            tokio::time::sleep(delay).await;
            if let Ok(Some(info)) = medulla::update::check_for_update(&url, current).await {
                let notice = format!("update v{} available — run `medulla update`", info.version);
                if tx.send(AppMsg::UpdateAvailable(notice)).is_err() {
                    break; // app exited
                }
            }
        }
    });
}

/// Translate a [`Cmd`] emitted by the app into a spawned async task whose result
/// is reported back over the [`AppMsg`] channel. Memory queries touch SQLite so
/// they run on `spawn_blocking` off the UI thread.
fn run_cmd(
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
fn read_memory(
    runtime: &Arc<dyn Runtime>,
    memory: Option<&medulla::memory::MemoryService>,
) -> (Option<medulla::memory::MemoryStatus>, Vec<String>) {
    match memory {
        Some(svc) => (Some(svc.status()), svc.directives()),
        None => (runtime.memory_status(), runtime.memory_directives()),
    }
}
