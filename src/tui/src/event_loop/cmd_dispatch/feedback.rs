//! Dispatch the feedback-board commands off the UI thread.
//!
//! Every board call is a backend round trip, so each one is spawned and reports
//! back over [`AppMsg`]. Kept out of the main dispatcher for the same reason the
//! task commands are: five arms of the same shape read better as one surface
//! than as a fifth of the match they would otherwise sit in.

use std::sync::Arc;

use medulla::runtime::Runtime;
use medulla_tui::ui::app::Cmd;

use super::super::AppMsg;

/// Spawn a feedback command, returning non-feedback commands to the caller.
pub(super) fn run_feedback_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) -> Option<Box<Cmd>> {
    match cmd {
        Cmd::LoadFeedback(query) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            // The query rides back with the answer: filter and sort changes can
            // overtake each other, and a response is only worth applying while
            // it still describes what the header says is on screen.
            let echoed = query.clone();
            tokio::spawn(async move {
                let msg = match rt.list_feedback(query).await {
                    Ok(page) => AppMsg::FeedbackLoaded {
                        query: echoed,
                        page,
                    },
                    Err(e) => AppMsg::Status(e.to_string()),
                };
                let _ = tx.send(msg);
            });
        }
        Cmd::LoadFeedbackDetail(id) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let msg = match rt.feedback_detail(id.clone()).await {
                    Ok(detail) => AppMsg::FeedbackComments {
                        id,
                        comments: detail.comments,
                    },
                    Err(e) => AppMsg::Status(e.to_string()),
                };
                let _ = tx.send(msg);
            });
        }
        Cmd::VoteFeedback { id, value } => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let msg = match rt.vote_feedback(id, value).await {
                    Ok(item) => AppMsg::FeedbackItemUpdated(item),
                    Err(e) => AppMsg::Status(e.to_string()),
                };
                let _ = tx.send(msg);
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
        other => return Some(Box::new(other)),
    }
    None
}
