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

mod cmd_dispatch;
mod types;
mod update_checker;

#[cfg(test)]
mod tests;

use types::AppMsg;
pub(crate) use types::{SessionExit, SessionWiring};
use update_checker::spawn_update_checker;

pub(crate) use cmd_dispatch::clear_copilot_hosts;
use cmd_dispatch::run_cmd;

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
        account,
        mut sharing,
        onboarding_path,
        host,
        harnesses,
    } = wiring;
    let mut app = App::new(runtime.clone(), loaded);
    app.set_config_path(config_path);
    app.set_medulla_home(medulla_home);
    app.set_account(account);
    if let Some(obs) = tinyplace_obs {
        app.set_tinyplace_observation(obs);
    }
    if let Some(host) = host {
        app.set_host_observation(host);
    }
    if let Some(harnesses) = harnesses {
        app.set_local_harnesses(harnesses);
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
                        run_cmd(cmd, &runtime, &app.loaded.config.workflows, &msg_tx);
                    }
                }
            }
            recv = sub.recv() => {
                if recv.is_ok() {
                    app.refresh_snapshot();
                    if should_refresh_context(&mut app) {
                        run_cmd(Cmd::InspectContext, &runtime, &app.loaded.config.workflows, &msg_tx);
                    }
                }
            }
            Some(msg) = msg_rx.recv() => {
                match msg {
                    AppMsg::Status(s) => { app.set_status(s); app.refresh_snapshot(); }
                    AppMsg::Contexts(c) => app.set_contexts(c),
                    AppMsg::UsageLoaded(data) => app.set_account_usage(data),
                    AppMsg::OpenResume(chats) => app.open_resume(chats),
                    AppMsg::LoggedOut => {
                        app.set_status("Account · logged out. Returning to the login screen…");
                        app.logged_out();
                    }
                    AppMsg::Resumed(s) => {
                        app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap_or(1);
                        app.refresh_snapshot();
                        app.set_status(s);
                    }
                    #[cfg(feature = "workflows")]
                    AppMsg::CopilotStarted { workflow, instruction } => {
                        app.copilot_started(&workflow, &instruction);
                    }
                    #[cfg(feature = "workflows")]
                    AppMsg::CopilotStatus { workflow, line } => {
                        app.copilot_status(&workflow, line);
                    }
                    #[cfg(feature = "workflows")]
                    AppMsg::CopilotDone {
                        workflow,
                        reply,
                        changes,
                        created,
                        removed,
                    } => {
                        // The conversation follows the workflow. A create turn
                        // ran on a sentinel thread because there was no id yet;
                        // now there is one, so the harness session moves onto it
                        // and the operator's follow-up is turn two of the same
                        // conversation rather than turn one of a new one.
                        if let Some(created) = &created {
                            cmd_dispatch::adopt_copilot_host(&workflow, created);
                        }
                        // Nothing left to continue, and no reason to keep a
                        // daemon and its harness processes alive for it.
                        if removed {
                            cmd_dispatch::close_copilot_host(&workflow);
                        }
                        // A queued follow-up comes back as a command to run:
                        // the drain happens after the catalogue refresh, so it
                        // sees the graph this turn left behind.
                        let queued = app.copilot_finished(&workflow, reply, changes, created);
                        if let Some(cmd) = queued {
                            run_cmd(cmd, &runtime, &app.loaded.config.workflows, &msg_tx);
                        }
                    }
                    #[cfg(feature = "workflows")]
                    AppMsg::CopilotFailed { workflow, error } => {
                        app.copilot_failed(&workflow, error);
                    }
                    #[cfg(feature = "workflows")]
                    AppMsg::WorkflowsChanged => app.reload_workflows(),
                    AppMsg::FeedbackLoaded { query, page } => {
                        if !app.set_feedback_page(query, page) {
                            continue;
                        }
                        // Pull the newly selected row's comments in the same beat.
                        if let Some(cmd) = app.feedback_detail_cmd() {
                            run_cmd(cmd, &runtime, &app.loaded.config.workflows, &msg_tx);
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
                        // The board is about to be re-read; forgetting which
                        // item's comments are loaded is what makes the reload
                        // fetch them again, so a just-posted comment appears.
                        app.invalidate_feedback_detail();
                        // A comment or submission changes the board, so re-pull
                        // it rather than patching state locally.
                        run_cmd(
                            Cmd::LoadFeedback(app.feedback_query()),
                            &runtime,
                            &app.loaded.config.workflows,
                            &msg_tx,
                        );
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

/// Detect a changed event stream while the nested Context surface is visible.
fn should_refresh_context(app: &mut App) -> bool {
    app.tab() == "Settings" && app.settings_subpage() == "Context" && app.events_changed()
}
