//! The interactive TUI event loop and its async plumbing.
//!
//! Owns [`AppMsg`] (the messages spawned tasks send back to the UI), the main
//! [`run`] select-loop over crossterm events / runtime pings / a 90ms tick,
//! and the background [`spawn_update_checker`]. The loop keeps all mutation on
//! one task and folds async results back in over an mpsc channel.
//!
//! Command dispatch lives in the sibling [`dispatch`] module so the main loop
//! stays focused on the select-loop choreography.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::EventStream;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, Cmd, TABS};

use crate::terminal::set_mouse_capture;

mod dispatch;
mod types;

#[cfg(test)]
mod tests;

use dispatch::run_cmd;
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
                    AppMsg::WorkspacesLoaded(reports) => {
                        app.set_workspace_reports(reports);
                        if app.tab() == "Repo" {
                            app.set_status("Repo · refreshed");
                            if let Some(cmd) = app.selected_repo_diff_cmd() {
                                run_cmd(cmd, &runtime, app.memory_service(), &msg_tx);
                            }
                        } else {
                            app.set_status("Agents · path claims refreshed");
                        }
                    }
                    AppMsg::WorkspaceDiffLoaded { workspace, path, result } => {
                        app.set_workspace_diff(workspace, path, result);
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
