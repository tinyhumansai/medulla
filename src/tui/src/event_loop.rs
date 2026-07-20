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

use medulla::runtime::{ContextItem, Runtime};
use medulla_tui::ui::app::{App, Cmd, TABS};

use crate::terminal::set_mouse_capture;

/// Messages sent from spawned async tasks back to the event loop.
pub(crate) enum AppMsg {
    Status(String),
    Contexts(Vec<ContextItem>),
    OpenResume(Vec<medulla::ui::chat_store::MainChatSummary>),
    Resumed(String),
    MemoryLoaded {
        status: Option<medulla::memory::MemoryStatus>,
        directives: Vec<String>,
    },
    UsageLoaded(Option<serde_json::Value>),
    MemoryResults {
        hits: Vec<medulla::memory::MemoryHit>,
        query: String,
    },
    /// A newer release was detected by the background update checker.
    UpdateAvailable(String),
    /// A page of the feedback board. `None` = this runtime has no board.
    FeedbackLoaded(Option<medulla::client::FeedbackPage>),
    /// Comments for one board item.
    FeedbackComments {
        /// The item the comments belong to.
        id: String,
        /// The item's comments, oldest first.
        comments: Vec<medulla::client::FeedbackComment>,
    },
    /// A board item the server re-tallied after a vote.
    FeedbackItemUpdated(medulla::client::FeedbackItem),
    /// A feedback action finished; reload the board and report `status`.
    FeedbackChanged(String),
}

/// Drive the ratatui app: build [`App`], subscribe to the runtime, and loop over
/// input events, runtime snapshots, background [`AppMsg`]s, and the animation
/// tick until the app requests quit.
pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    runtime: Arc<dyn Runtime>,
    loaded: medulla::config::LoadedConfig,
    startup_status: Option<String>,
    tinyplace_obs: Option<Arc<std::sync::Mutex<medulla::tinyplace::service::TinyplaceObservation>>>,
    config_path: std::path::PathBuf,
    medulla_home: std::path::PathBuf,
) -> anyhow::Result<()> {
    let mut app = App::new(runtime.clone(), loaded);
    app.set_config_path(config_path);
    app.set_medulla_home(medulla_home);
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
                        run_cmd(cmd, &runtime, &msg_tx);
                    }
                }
            }
            recv = sub.recv() => {
                if recv.is_ok() {
                    app.refresh_snapshot();
                    if app.tab() == "Context" && app.events_changed() {
                        run_cmd(Cmd::InspectContext, &runtime, &msg_tx);
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
                    AppMsg::MemoryResults { hits, query } => {
                        let n = hits.len();
                        app.set_memory_results(hits, query);
                        app.set_status(format!("Memory · {n} hit(s)"));
                    }
                    AppMsg::FeedbackLoaded(page) => {
                        app.set_feedback_page(page);
                        // Pull the newly selected row's comments in the same beat.
                        if let Some(cmd) = app.feedback_detail_cmd() {
                            run_cmd(cmd, &runtime, &msg_tx);
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
                        run_cmd(Cmd::LoadFeedback(app.feedback_query()), &runtime, &msg_tx);
                    }
                    AppMsg::UpdateAvailable(notice) => {
                        app.set_update_notice(notice.clone());
                        app.set_status(notice);
                        app.refresh_snapshot();
                    }
                }
            }
            _ = tick.tick() => {
                if app.snapshot.running {
                    app.frame = app.frame.wrapping_add(1);
                }
            }
        }
    }
    Ok(())
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
                let status = rt.memory_status();
                let directives = rt.memory_directives();
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
            });
        }
        Cmd::SearchMemory(query) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let status = rt.memory_status();
                let directives = rt.memory_directives();
                let hits = rt.memory_search(query.clone(), None, 20);
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
                let _ = tx.send(AppMsg::MemoryResults { hits, query });
            });
        }
    }
}
