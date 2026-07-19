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
//!
//! The loop itself lives in [`drive_welcome_ui`], which is generic over the
//! ratatui backend and takes its key events as a stream. [`run_welcome_ui`] is
//! the thin production wrapper that supplies the real terminal and crossterm's
//! event stream; tests drive the same loop with a test backend and a scripted
//! stream, so the flow is exercised end to end without a TTY.

use std::collections::HashMap;
use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::{Stream, StreamExt};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;
use tokio::sync::mpsc::UnboundedSender;

use medulla::client::MedullaClient;
use medulla::history_upload::{scan_local_history, share_history, HistoryScan};

use super::types::{
    ScanSummary, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen, DEFAULT_MAX_REWARD_USD,
};

/// Render the welcome flow and return what the user chose.
///
/// Returns immediately with [`WelcomeOutcome::Skipped`] when the backend says the
/// reward was already granted — the backend, not the local config flag, is the
/// authority on that.
///
/// A transport failure while checking returns [`WelcomeOutcome::Unavailable`]
/// instead: the user must never be blocked from the app by this optional flow,
/// but nor should a transient backend error permanently burn their offer.
///
/// Nothing is uploaded before the user explicitly approves the consent step.
pub async fn run_welcome_ui(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &MedullaClient,
    env: HashMap<String, String>,
) -> anyhow::Result<WelcomeOutcome> {
    // Crossterm's raw event stream, narrowed to the key presses the screen cares
    // about, so the loop itself never touches terminal types.
    let keys = EventStream::new().filter_map(|event| async move {
        match event {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => Some(key),
            _ => None,
        }
    });
    futures::pin_mut!(keys);
    drive_welcome_ui(terminal, client, env, keys).await
}

/// The welcome loop, generic over the backend and its key source.
///
/// Split out from [`run_welcome_ui`] so the whole flow — scanning, consent,
/// uploading, claiming, and every render — can be driven headlessly in tests.
/// Production code should call [`run_welcome_ui`].
///
/// Returns immediately with [`WelcomeOutcome::Skipped`] when the backend says the
/// reward was already granted — the backend, not the local config flag, is the
/// authority on that. Any transport failure while checking is also treated as
/// "skip": a new user must never be blocked from the app by this optional flow.
///
/// Nothing is uploaded before the user explicitly approves the consent step.
pub async fn drive_welcome_ui<B, K>(
    terminal: &mut Terminal<B>,
    client: &MedullaClient,
    env: HashMap<String, String>,
    mut keys: K,
) -> anyhow::Result<WelcomeOutcome>
where
    B: Backend,
    K: Stream<Item = KeyEvent> + Unpin,
{
    let max_reward_usd = match client.history_reward_status().await {
        Ok(status) if status.claimed => return Ok(WelcomeOutcome::Skipped),
        Ok(status) => status.max_reward_usd,
        Err(_) => return Ok(WelcomeOutcome::Unavailable),
    };

    let mut screen = WelcomeScreen::new(if max_reward_usd > 0.0 {
        max_reward_usd
    } else {
        DEFAULT_MAX_REWARD_USD
    });
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WelcomeEvent>();

    // Owned by the driver, populated by the scan command, consumed by the upload.
    let scan: Arc<tokio::sync::Mutex<HistoryScan>> =
        Arc::new(tokio::sync::Mutex::new(HistoryScan::default()));
    // The task servicing the current command. Held so that leaving the flow can
    // abort it — without this, skipping mid-upload would return while a detached
    // task kept uploading transcripts and went on to claim the reward.
    let mut inflight: Option<tokio::task::JoinHandle<()>> = None;

    let outcome = loop {
        terminal.draw(|f| screen.draw(f))?;
        if let Some(outcome) = screen.outcome() {
            break outcome;
        }

        tokio::select! {
            Some(key) = keys.next() => {
                if let Some(cmd) = screen.handle_key(key) {
                    inflight = Some(dispatch(cmd, client, &env, &scan, &tx));
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    };

    // Withdrawing consent must actually stop the work: anything still uploading
    // is cancelled here, and with it the claim that would have followed.
    if let Some(handle) = inflight {
        handle.abort();
    }

    Ok(outcome)
}

/// Spawn the async work a [`WelcomeCmd`] requires, returning its handle so the
/// caller can cancel it when the flow ends.
fn dispatch(
    cmd: WelcomeCmd,
    client: &MedullaClient,
    env: &HashMap<String, String>,
    scan: &Arc<tokio::sync::Mutex<HistoryScan>>,
    tx: &UnboundedSender<WelcomeEvent>,
) -> tokio::task::JoinHandle<()> {
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
            })
        }
        WelcomeCmd::UploadAndClaim => {
            let client = client.clone();
            let scan = scan.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                upload_and_claim(client, scan, tx).await;
            })
        }
    }
}

/// Redact, upload, and claim, forwarding the SDK's progress as screen events.
///
/// The orchestration itself lives in [`medulla::history_upload::share_history`] —
/// it is UI-free logic and is tested there against a mock backend. This wrapper
/// only translates progress and the final claim into [`WelcomeEvent`]s.
async fn upload_and_claim(
    client: MedullaClient,
    scan: Arc<tokio::sync::Mutex<HistoryScan>>,
    tx: UnboundedSender<WelcomeEvent>,
) {
    let files = scan.lock().await.files.clone();

    let progress_tx = tx.clone();
    let claimed = share_history(&client, &files, move |progress| {
        let _ = progress_tx.send(WelcomeEvent::UploadProgress {
            uploaded: progress.uploaded,
            total: progress.total,
            redactions: progress.redactions,
        });
    })
    .await;

    let event = match claimed {
        Ok(claim) => WelcomeEvent::Claimed {
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
        },
        Err(err) => WelcomeEvent::Failed(format!("could not claim reward: {err}")),
    };
    let _ = tx.send(event);
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
