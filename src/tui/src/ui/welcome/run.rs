//! The interactive welcome driver: [`run_welcome_ui`] drives the
//! [`WelcomeScreen`] state machine and services its async commands (scanning
//! local history, then redacting/uploading/claiming) until the screen reaches an
//! outcome.
//!
//! Like [`crate::ui::login`], it borrows the app's already-configured terminal
//! rather than setting up its own: the welcome flow runs inside the TUI's
//! alt-screen session, between login and the main event loop.
//!
//! The driver owns the scanned [`HistoryScan`] — the screen only ever holds
//! display numbers — so transcript contents never enter the pure state machine.

use std::collections::HashMap;
use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::UnboundedSender;

use medulla::client::MedullaClient;
use medulla::history_upload::{read_redacted_session, scan_local_history, HistoryScan};

use super::types::{
    ScanSummary, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen, DEFAULT_MAX_REWARD_USD,
};

/// Render the welcome flow and return what the user chose.
///
/// Returns immediately with [`WelcomeOutcome::Skipped`] when the backend says the
/// reward was already granted — the backend, not the local config flag, is the
/// authority on that. Any transport failure while checking is also treated as
/// "skip": a new user must never be blocked from the app by this optional flow.
///
/// Nothing is uploaded before the user explicitly approves the consent step.
pub async fn run_welcome_ui(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &MedullaClient,
    env: HashMap<String, String>,
) -> anyhow::Result<WelcomeOutcome> {
    let max_reward_usd = match client.history_reward_status().await {
        Ok(status) if status.claimed => return Ok(WelcomeOutcome::Skipped),
        Ok(status) => status.max_reward_usd,
        Err(_) => return Ok(WelcomeOutcome::Skipped),
    };

    let mut screen = WelcomeScreen::new(if max_reward_usd > 0.0 {
        max_reward_usd
    } else {
        DEFAULT_MAX_REWARD_USD
    });
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WelcomeEvent>();

    // Owned by the driver, populated by the scan command, consumed by the upload.
    let scan: Arc<tokio::sync::Mutex<HistoryScan>> =
        Arc::new(tokio::sync::Mutex::new(HistoryScan::default()));

    let outcome = loop {
        terminal.draw(|f| screen.draw(f))?;
        if let Some(outcome) = screen.outcome() {
            break outcome;
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind != KeyEventKind::Release {
                        if let Some(cmd) = screen.handle_key(key) {
                            dispatch(cmd, client, &env, &scan, &tx);
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    };

    Ok(outcome)
}

/// Spawn the async work a [`WelcomeCmd`] requires.
fn dispatch(
    cmd: WelcomeCmd,
    client: &MedullaClient,
    env: &HashMap<String, String>,
    scan: &Arc<tokio::sync::Mutex<HistoryScan>>,
    tx: &UnboundedSender<WelcomeEvent>,
) {
    match cmd {
        WelcomeCmd::Scan => {
            let env = env.clone();
            let scan = scan.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                // Blocking filesystem work off the reactor thread.
                let found = tokio::task::spawn_blocking(move || scan_local_history(&env))
                    .await
                    .unwrap_or_default();
                let summary = summarize(&found);
                *scan.lock().await = found;
                let _ = tx.send(WelcomeEvent::ScanReady(summary));
            });
        }
        WelcomeCmd::UploadAndClaim => {
            let client = client.clone();
            let scan = scan.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                upload_and_claim(client, scan, tx).await;
            });
        }
    }
}

/// Redact and upload each transcript, then claim the reward.
///
/// A single transcript that fails to read or upload is skipped rather than
/// aborting: a partial share still earns credit for what did land, which is
/// strictly better for the user than failing the whole flow.
async fn upload_and_claim(
    client: MedullaClient,
    scan: Arc<tokio::sync::Mutex<HistoryScan>>,
    tx: UnboundedSender<WelcomeEvent>,
) {
    let files = scan.lock().await.files.clone();
    let total = files.len();
    let mut uploaded = 0usize;
    let mut redactions = 0usize;

    for file in files {
        // Reading and redacting is CPU/IO bound; keep it off the reactor.
        let session = tokio::task::spawn_blocking(move || read_redacted_session(&file))
            .await
            .ok()
            .flatten();
        let Some(session) = session else { continue };

        let agent = session.agent.as_str();
        match client
            .upload_history_session(agent, session.content.clone())
            .await
        {
            Ok(_) => {
                uploaded += 1;
                redactions += session.redactions;
                let _ = tx.send(WelcomeEvent::UploadProgress {
                    uploaded,
                    total,
                    redactions,
                });
            }
            Err(_) => {
                // One transcript failing is not fatal: the claim below reports
                // the real server-side state, including the "already settled"
                // case where the backend refuses further uploads outright.
                let _ = tx.send(WelcomeEvent::UploadProgress {
                    uploaded,
                    total,
                    redactions,
                });
            }
        }
    }

    match client.claim_history_reward().await {
        Ok(claim) => {
            let _ = tx.send(WelcomeEvent::Claimed {
                awarded_usd: claim.status.awarded_usd,
                tier: claim.status.tier.clone(),
                breakdown: vec![
                    ("token volume".into(), claim.breakdown.tokens_usd),
                    ("active days".into(), claim.breakdown.active_days_usd),
                    ("sessions".into(), claim.breakdown.sessions_usd),
                    ("multi-agent".into(), claim.breakdown.multi_agent_usd),
                ],
                max_reward_usd: claim.status.max_reward_usd,
                already_claimed: claim.already_claimed,
            });
        }
        Err(err) => {
            let _ = tx.send(WelcomeEvent::Failed(format!(
                "could not claim reward: {err}"
            )));
        }
    }
}

/// Project a scan into the display-only summary the screen holds.
fn summarize(scan: &HistoryScan) -> ScanSummary {
    ScanSummary {
        per_agent: scan
            .tallies()
            .into_iter()
            .map(|tally| (tally.agent.as_str().to_string(), tally.session_count))
            .collect(),
        session_count: scan.session_count(),
        total_bytes: scan.total_bytes(),
        skipped_oversize: scan.skipped_oversize,
    }
}
