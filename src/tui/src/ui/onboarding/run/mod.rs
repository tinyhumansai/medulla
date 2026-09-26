//! The interactive onboarding driver: [`run_onboarding_ui`] sets up a minimal
//! terminal, drives the [`OnboardingScreen`] state machine, and services its
//! identity command until the screen reaches an outcome. This is the app-side
//! rendering the SDK's
//! [`medulla::onboarding::ensure_registered`] injects as an
//! [`OnboardingUi`](medulla::onboarding::OnboardingUi) callback.

use std::io::{self, Stdout, Write};
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::onboarding::OnboardingContext;

use super::types::{OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen};

/// Render the interactive onboarding screen and return the operator's choice.
///
/// Builds a minimal alt-screen terminal, drives the [`OnboardingScreen`] loop,
/// and answers its identity command from the link identity the SDK already
/// resolved.
/// Returns `Some((name, owner))` to register or `None` when the operator aborts
/// (q / Ctrl-C). This is the concrete implementation the app wraps into an
/// `OnboardingUi` for `ensure_registered`.
pub async fn run_onboarding_ui(
    ctx: OnboardingContext,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    let OnboardingContext {
        default_name,
        prefill_owner,
        endpoint,
        address,
    } = ctx;

    let mut guard = OnboardTermGuard::setup()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut screen = OnboardingScreen::new(default_name, prefill_owner, endpoint.clone());
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OnboardingEvent>();

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
                            dispatch(cmd, &address, &tx);
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    };

    guard.restore();

    Ok(match outcome {
        OnboardingOutcome::Register { name, owner } => Some((name, owner)),
        OnboardingOutcome::Abort => None,
    })
}

/// Answer an [`OnboardingCmd`]. The only command is the identity step, and the
/// host link has no directory to look a handle up in — enrollment issues the
/// node name and the id together (protocol §2) — so the address the SDK
/// resolved is the whole answer.
fn dispatch(
    cmd: OnboardingCmd,
    address: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<OnboardingEvent>,
) {
    match cmd {
        OnboardingCmd::LoadIdentity { .. } => {
            let _ = tx.send(OnboardingEvent::IdentityReady {
                address: address.to_string(),
                handle: None,
            });
        }
    }
}

impl OnboardTermGuard {
    /// Enter raw mode + the alternate screen, restoring on drop.
    fn setup() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen)?;
        out.flush()?;
        Ok(OnboardTermGuard { active: true })
    }

    /// Leave the alternate screen and disable raw mode (idempotent).
    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut out: Stdout = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = out.flush();
    }
}

impl Drop for OnboardTermGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

mod types;
use types::OnboardTermGuard;
