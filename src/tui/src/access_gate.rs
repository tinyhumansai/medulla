//! The launch-time access check: may this account use Medulla?
//!
//! Split out of [`crate::app_loop`] for the same reason [`crate::sign_in`] was:
//! it is a separate question from wiring up the app. Signing in establishes
//! *who* the operator is; this establishes whether that account is entitled to
//! what it just signed in to.
//!
//! # Why it runs on every launch, not just after a login
//!
//! Entitlement changes without the operator touching the terminal — a trial runs
//! out overnight, a card is declined, a subscription is cancelled from the web.
//! A check that only ran at sign-in would admit an account that stopped being
//! entitled weeks ago, because the session it signed in with is still perfectly
//! valid. So it hangs off the boot path, after a session is resolved, and asks
//! the backend fresh each time.
//!
//! # A backend it cannot reach is a refusal
//!
//! Entitlement is not something this binary can decide alone, so a launch that
//! cannot ask does not get to assume. The operator is shown the same refusal as
//! an unresolvable plan, phrased as what it is — we could not confirm — rather
//! than as an expired trial.
//!
//! # The refusal is re-checkable, and escapable
//!
//! A refusal is not the end of the launch. The screen can re-ask the backend
//! ([`UpgradeOutcome::CheckAgain`]) so an operator who has just subscribed is
//! admitted without relaunching, and it can end the session
//! ([`UpgradeOutcome::SignOut`]) so an operator signed in to the wrong account
//! can reach the login screen — which otherwise only runs when there is no
//! session at all. Neither weakens the gate: the first is admitted only when
//! this same policy, run again on a fresh `/auth/me`, says yes.
//!
//! # What this is not
//!
//! It is not the enforcement point — see [`medulla::access`]. What an account
//! may actually spend is metered by the backend on the routes that cost money,
//! where a patched client cannot argue. This decides what the terminal shows,
//! and its job is to explain the situation rather than to hold a line.

use std::collections::HashMap;
use std::io;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::access::{Access, Denial, Grant};
use medulla::auth::{describe_me, open_browser};
use medulla::client::MedullaClient;
use medulla_tui::ui::upgrade::{UpgradeCmd, UpgradeOutcome, UpgradeScreen};

/// What the gate decided about this launch.
pub(crate) enum Gate {
    /// Carry on into the app. Carries the startup note to show, when the grant
    /// is one worth mentioning — a trial counting down.
    Allowed(Option<String>),
    /// Stop. Carries what to tell the operator once the terminal is theirs
    /// again — nothing at all when they simply quit.
    Refused(Option<String>),
}

/// Check `session`'s entitlement and, when it is refused, show the gate screen.
///
/// Runs inside the caller's alt-screen session, borrowing the terminal the same
/// way the login screen does, so there is no flicker between them.
///
/// `env` and `backend` are only consulted when the operator signs out from the
/// screen: the first locates the session store to clear, the second says whether
/// a token from the config or the environment would keep them signed in anyway.
///
/// # Errors
///
/// Propagates terminal draw failures only. A backend that cannot be reached is
/// not an error either — it is a refusal, shown as [`Denial::Undetermined`]:
/// a launch that could not ask whether this account is entitled does not get to
/// assume that it is.
pub(crate) async fn check(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    env: &HashMap<String, String>,
    backend: &medulla::config::BackendConfig,
    session: &medulla::auth::Credentials,
) -> anyhow::Result<Gate> {
    let client = MedullaClient::new(session.base_url.clone(), session.jwt.clone());
    let (who, denial) = match client.me().await {
        // Unreachable, rate-limited, 500. None of them say this account pays,
        // and none of them say its free window is up either — so it is refused
        // as the unconfirmed case, under the deployment's name, which is the
        // only identity still in hand.
        Err(_) => (who(session), Denial::Undetermined),
        Ok(me) => match medulla::access::access_from_me(&me) {
            Access::Granted(grant) => return Ok(Gate::Allowed(startup_note(grant))),
            Access::Denied(denial) => (describe_me(&me), denial),
        },
    };

    run_upgrade_screen(terminal, &client, who, denial)
        .await?
        .resolve(env, backend)
}

/// Run [`check`] and fold its outcome into the caller's boot sequence.
///
/// [`Gate::Refused`] means the caller should return immediately: the gate screen
/// has already run, and the terminal guard the caller still holds restores it
/// the same way every other early return in [`crate::app_loop::run_tui`] does,
/// by going out of scope. Its payload is a parting message for the caller to
/// print once that has happened.
///
/// A missing `session` — the `--mock` demo, which never reaches a backend — is
/// not asked at all, since there is nothing to bill.
pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    env: &HashMap<String, String>,
    backend: &medulla::config::BackendConfig,
    session: Option<&medulla::auth::Credentials>,
) -> anyhow::Result<Gate> {
    let Some(session) = session else {
        return Ok(Gate::Allowed(None));
    };
    check(terminal, env, backend, session).await
}

/// The best "who am I" line available without a backend to ask.
///
/// `/auth/me` is what normally phrases this, so when that is the call that
/// failed there is nothing to name the operator with — the deployment they were
/// talking to is the one fact still in hand, and it is the one that matters for
/// telling a staging mix-up from a real outage.
fn who(session: &medulla::auth::Credentials) -> String {
    format!("backend {}", session.base_url)
}

/// The one-line startup note a grant is worth announcing with, if any.
///
/// Only the trial gets one, and only because it is the grant with a deadline:
/// an operator whose free window ends on Thursday should not discover that on
/// Thursday. A paid or allowlisted account needs no notice that it is fine.
fn startup_note(grant: Grant) -> Option<String> {
    let Grant::Trial { remaining_ms } = grant else {
        return None;
    };
    // Round up: with eleven hours left it is more useful — and more honest — to
    // say "1 day left" than "0 days left".
    let day_ms = 24 * 60 * 60 * 1_000_i64;
    let days = remaining_ms.div_euclid(day_ms) + i64::from(remaining_ms.rem_euclid(day_ms) > 0);
    let left = if days == 1 {
        "1 day".to_string()
    } else {
        format!("{days} days")
    };
    // Says what happens next, not just how long is left: the note is the only
    // place a trialling operator is told a subscription is coming for them.
    Some(format!(
        "Free trial: {left} left. Medulla needs a Basic subscription after that — \
         https://tinyhumans.ai/pricing"
    ))
}

/// How the gate screen ended, before the side effects it implies are carried out.
enum Screen {
    /// The re-check succeeded and the account is in after all. Carries the
    /// startup note, exactly as the first-time grant would.
    Admitted(Option<String>),
    /// The operator chose to leave.
    Quit,
    /// The operator chose to sign out. The session is still on disk at this
    /// point — see [`Screen::resolve`].
    SignOut,
}

impl Screen {
    /// Carry out what the outcome implies and phrase it as a [`Gate`].
    ///
    /// Signing out happens here, after the screen has stopped drawing, rather
    /// than inside its loop: clearing the session is the caller's job in the
    /// same way opening a browser is, and doing it here keeps the screen a pure
    /// state machine.
    fn resolve(
        self,
        env: &HashMap<String, String>,
        backend: &medulla::config::BackendConfig,
    ) -> anyhow::Result<Gate> {
        match self {
            Screen::Admitted(note) => Ok(Gate::Allowed(note)),
            Screen::Quit => Ok(Gate::Refused(None)),
            Screen::SignOut => Ok(Gate::Refused(Some(sign_out(env, backend)))),
        }
    }
}

/// Forget the stored session, and say what that did and did not achieve.
///
/// Never fails the launch: the operator is already on their way out, and an
/// error here is news to report rather than a reason to replace a clean exit
/// with a stack trace. It is reported *as* an error, though — telling somebody
/// they are signed out while the bearer is still on disk is the one outcome a
/// security action must never produce.
fn sign_out(env: &HashMap<String, String>, backend: &medulla::config::BackendConfig) -> String {
    // The account id lives in the session, so it is read before the clear;
    // the event is sent only once the clear succeeded, since a session that
    // survives is not a sign-out. Only when the stored session is what
    // authenticated this run, or the event would name the wrong account.
    let signed_out_user = (!medulla::auth::external_token_wins(env, backend))
        .then(|| medulla::auth::state(env).user_id)
        .flatten();
    if let Err(e) = medulla::auth::clear(env) {
        return format!("The stored session could not be removed: {e}");
    }
    // As every other sign-out does: a crash from here on is not that account's.
    medulla::observability::set_user(None);
    if let Some(user_id) = signed_out_user {
        medulla::analytics::spawn_tracked(async move {
            let _ = medulla::analytics::record_sign_out(&user_id).await;
        });
    }
    // Retired credential files go too, for the reason `medulla logout` gives:
    // the next launch would adopt one and sign the operator straight back in to
    // the account they just left. A file that survives the removal (read-only
    // mount, permissions) is exactly the bearer that would do that, so it is
    // named in the parting message rather than silently left behind.
    let home = medulla::home::medulla_home(env);
    let undeleted: Vec<_> = medulla::auth::adopt_legacy_credentials(env, &home)
        .into_iter()
        .filter(|legacy| !medulla::auth::discard_legacy_credential(&legacy.path))
        .map(|legacy| legacy.path)
        .collect();
    // An inline `backend.token` or a `backend.tokenEnv` variable outranks the
    // store, so clearing it leaves the next process authenticated as the same
    // account — and landing back on this same screen would look like the sign
    // out silently failed. Both this and an undeleted legacy file are surviving
    // authentication sources, and either one alone would sign the operator
    // straight back in — so when both apply, both have to be named, or fixing
    // the one the operator sees still leaves them signed back in by the other.
    let mut warnings = Vec::new();
    if medulla::auth::resolve_backend_token(env, backend, None).is_some() {
        let source = if backend.token.is_some() {
            "`backend.token` in the config".to_string()
        } else {
            format!("the `{}` environment variable", backend.token_env)
        };
        warnings.push(format!(
            "this shell is still authenticated from {source} — remove it, then run `medulla` \
             to sign in as another account"
        ));
    }
    if !undeleted.is_empty() {
        let paths = undeleted
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "a legacy credential file could not be removed: {paths} — delete it by hand, or a \
             future launch may sign back in from it"
        ));
    }
    if warnings.is_empty() {
        return "Signed out. Run `medulla` to sign in again.".to_string();
    }
    format!("Signed out, but {}.", warnings.join("; and "))
}

/// Draw the gate screen until the operator leaves it or is admitted.
///
/// The re-check is driven from here rather than from the screen because the
/// screen holds no client: it asks, by resolving to
/// [`UpgradeOutcome::CheckAgain`], and this loop answers with the *same* policy
/// the launch check used. There is no path from a keystroke to the app that does
/// not go through [`medulla::access::access_from_me`].
async fn run_upgrade_screen(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &MedullaClient,
    who: String,
    denial: Denial,
) -> anyhow::Result<Screen> {
    use crossterm::event::{Event, EventStream, KeyEventKind};
    use futures::StreamExt;

    let mut screen = UpgradeScreen::new(who, denial);
    let mut reader = EventStream::new();

    loop {
        // Drawn before the outcome is taken, so the "Checking…" frame a re-check
        // sets is on screen for the length of the round trip below rather than
        // after it.
        terminal.draw(|f| screen.draw(f))?;
        match screen.take_outcome() {
            Some(UpgradeOutcome::Quit) => return Ok(Screen::Quit),
            Some(UpgradeOutcome::SignOut) => return Ok(Screen::SignOut),
            Some(UpgradeOutcome::CheckAgain) => {
                match client.me().await {
                    Ok(me) => match medulla::access::access_from_me(&me) {
                        Access::Granted(grant) => return Ok(Screen::Admitted(startup_note(grant))),
                        Access::Denied(denial) => screen.rechecked(describe_me(&me), denial),
                    },
                    // Nothing was learned about the account, so the panel keeps
                    // saying what it said and reports the failure as its own.
                    Err(e) => screen.recheck_failed(&e.to_string()),
                }
                continue;
            }
            None => {}
        }
        if let Some(Ok(Event::Key(key))) = reader.next().await {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            // Best-effort, like the login screen's link rows: a browser that
            // will not launch must not trap the operator on this screen.
            if let Some(UpgradeCmd::OpenUrl(url)) = screen.handle_key(key) {
                open_browser(&url);
            }
        }
    }
}

#[cfg(test)]
#[path = "access_gate_tests.rs"]
mod tests;
