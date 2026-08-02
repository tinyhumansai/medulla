//! The pre-app login screen driver.
//!
//! Sits between runtime selection and the app: when the embedded core reports
//! that nobody is signed in, the TUI draws [`LoginScreen`] here, runs the async
//! work its commands ask for (loopback OAuth, one-time-token redemption, JWT
//! verification), and returns a [`LoginOutcome`] the caller turns into a signed-in
//! core or a clean exit.
//!
//! The screen itself is a pure state machine in [`medulla_tui::ui::login`]; this
//! module is only its I/O — the terminal, the key stream, and the spawned tasks.

use std::io::Stdout;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::auth::{
    describe_me, is_one_time_login_token, open_browser, start_loopback, DEFAULT_LOGIN_TIMEOUT,
};
use medulla::client::MedullaClient;
use medulla_tui::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

/// Draw the [`LoginScreen`], route keys to async tasks, and fold their events
/// back in until the screen reaches an outcome.
///
/// Runs inside the caller's alt-screen session — it borrows the terminal rather
/// than setting one up, so the screen and the app share one surface and there is
/// no flicker between them.
///
/// # Errors
///
/// Propagates terminal draw failures. A failed sign-in is not an error: it stays
/// on the screen with a message, which is the whole point of the screen.
pub(crate) async fn run_login_screen(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    base_url: String,
) -> anyhow::Result<LoginOutcome> {
    let mut screen = LoginScreen::new(base_url.clone());
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LoginEvent>();
    let mut loopback_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        terminal.draw(|f| screen.draw(f))?;
        if let Some(outcome) = screen.outcome() {
            if let Some(h) = loopback_task.take() {
                h.abort();
            }
            return Ok(outcome);
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind != KeyEventKind::Release {
                        if let Some(cmd) = screen.handle_key(key) {
                            dispatch_login_cmd(cmd, &base_url, &tx, &mut loopback_task);
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    }
}

/// Spawn the async work a [`LoginCmd`] requires and stream results back as
/// [`LoginEvent`]s.
fn dispatch_login_cmd(
    cmd: LoginCmd,
    base_url: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<LoginEvent>,
    loopback_task: &mut Option<tokio::task::JoinHandle<()>>,
) {
    match cmd {
        LoginCmd::StartLoopback { base_url, provider } => {
            let tx = tx.clone();
            let handle = tokio::spawn(async move {
                match start_loopback(&base_url, provider).await {
                    Ok(lb) => {
                        let _ = tx.send(LoginEvent::LoopbackStarted {
                            url: lb.login_url().to_string(),
                            port: lb.port(),
                        });
                        open_browser(lb.login_url());
                        match lb.await_callback(DEFAULT_LOGIN_TIMEOUT).await {
                            Ok(jwt) => {
                                let _ = tx.send(LoginEvent::CallbackToken(jwt.clone()));
                                verify_and_emit(&base_url, jwt, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx.send(LoginEvent::CallbackError(e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(LoginEvent::CallbackError(e.to_string()));
                    }
                }
            });
            if let Some(old) = loopback_task.replace(handle) {
                old.abort();
            }
        }
        LoginCmd::CancelLoopback => {
            if let Some(h) = loopback_task.take() {
                h.abort();
            }
        }
        // Best-effort, like the login-URL opener: a browser that refuses to
        // launch must not interrupt a sign-in the user is part-way through.
        LoginCmd::OpenUrl(url) => open_browser(&url),
        LoginCmd::SubmitToken(token) => {
            let base = base_url.to_string();
            let tx = tx.clone();
            tokio::spawn(async move {
                let jwt = if is_one_time_login_token(&token) {
                    let client = MedullaClient::new(base.clone(), String::new());
                    match client.consume_login_token(token).await {
                        Ok(j) => j,
                        Err(e) => {
                            let _ = tx.send(LoginEvent::VerifyFailed(format!(
                                "login token redemption failed: {e}"
                            )));
                            return;
                        }
                    }
                } else {
                    token
                };
                verify_and_emit(&base, jwt, &tx).await;
            });
        }
    }
}

/// Verify a JWT via `me()` and emit the matching [`LoginEvent`].
///
/// Verification happens before the token ever reaches the core: a JWT that
/// `/auth/me` rejects would be refused by `auth_store_session` anyway, and
/// failing here keeps the operator on the screen that can fix it.
async fn verify_and_emit(
    base_url: &str,
    jwt: String,
    tx: &tokio::sync::mpsc::UnboundedSender<LoginEvent>,
) {
    let client = MedullaClient::new(base_url.to_string(), jwt.clone());
    match client.me().await {
        Ok(me) => {
            let _ = tx.send(LoginEvent::Verified {
                who: describe_me(&me),
                jwt,
            });
        }
        Err(e) => {
            let _ = tx.send(LoginEvent::VerifyFailed(format!(
                "verification failed: {e}"
            )));
        }
    }
}
