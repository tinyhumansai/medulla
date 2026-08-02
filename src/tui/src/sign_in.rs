//! Signing in, and keeping a session with the account whose home holds it.
//!
//! Split out of [`crate::app_loop`] because it is a separate responsibility from
//! wiring up the app: resolving *who* this process runs as, and refusing to put
//! one account's credential in another account's directory.
//!
//! # The ordering that matters
//!
//! Medulla scopes everything to `<root>/<account>` (see [`medulla::home`]), and
//! the embedded core's credential store lives inside one of those directories.
//! So a token can only be stored once its account is known to match the home the
//! core was booted against — which means asking the *backend* who it belongs to,
//! since the core can only answer that after the store being guarded.

use std::io;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::commands::run_login_screen;
use crate::terminal::TermGuard;
use medulla_tui::ui::login::LoginOutcome;

/// What the operator is told when a login lands on a different account.
///
/// The session was deliberately not stored (see [`store_if_same_account`]), so
/// the next launch signs in against that account's own home — which is where its
/// credential belongs.
pub(crate) const SWITCHED_ACCOUNT_NOTICE: &str =
    "Signed in as a different account. Restart medulla to finish signing in as them.";

/// What a login inside a running app resolved to.
pub(crate) enum SignIn {
    /// The operator quit the screen without signing in.
    Quit,
    /// The same account this process is running as, and its session is stored.
    SameAccount,
    /// A different account. Nothing was stored — see [`store_if_same_account`].
    /// Carries the deployment-mismatch notice, when there is one to report once
    /// the terminal belongs to the caller again.
    SwitchedAccount(Option<String>),
}

/// Run the login screen again for an already-booted core, and store the session
/// only if it belongs to the account this process is running as.
///
/// A core that rejects the freshly verified token is an error rather than a
/// silent return to the screen: the token passed `/auth/me`, so a refusal here
/// means the core and the login flow disagree about which deployment they are
/// talking to, and looping would just ask for the same token again.
pub(crate) async fn relogin(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    core: &medulla::core_host::EmbeddedCore,
    env: &std::collections::HashMap<String, String>,
    base_url: &str,
) -> anyhow::Result<SignIn> {
    match run_login_screen(terminal, base_url.to_string()).await? {
        LoginOutcome::Quit => Ok(SignIn::Quit),
        LoginOutcome::Token(jwt) => store_if_same_account(core, env, base_url, &jwt).await,
    }
}

/// Hand a freshly verified token to the core — but only when it belongs to the
/// account whose home this core was booted against.
///
/// The identity check has to come *before* the store, not after. This core's
/// credential store lives inside one account's home, so storing first would
/// write account B's bearer token into account A's directory: the operator ends
/// up with a session they cannot reach (B's home has none) and a credential in a
/// directory that is not theirs. Nothing is stored on a switch — the marker
/// moves, the app stops, and the next launch signs in against B's own home.
///
/// The id comes from `/auth/me` rather than from the core's auth state because
/// the core can only answer that question *after* the store this guards.
///
/// # Errors
///
/// The backend cannot say whose token this is, or the core rejects a session
/// that does belong to this account.
async fn store_if_same_account(
    core: &medulla::core_host::EmbeddedCore,
    env: &std::collections::HashMap<String, String>,
    base_url: &str,
    jwt: &str,
) -> anyhow::Result<SignIn> {
    let me = medulla::client::MedullaClient::new(base_url.to_string(), jwt.to_string())
        .me()
        .await
        .map_err(|e| anyhow::anyhow!("signed in, but the account could not be read: {e}"))?;
    let root = medulla::home::medulla_root(env);
    let active = medulla::home::user::active_user_id(env, &root);
    // `MEDULLA_USER` selects an account for this process only, so a login under
    // it must never move the selection every other process reads — the same rule
    // the CLI adoption path follows.
    let overridden = env
        .get(medulla::home::user::MEDULLA_USER_ENV)
        .map(|v| v.trim())
        .is_some_and(|v| !v.is_empty());

    match disposition(
        &active,
        overridden,
        medulla::auth::user_id_from_me(&me).as_deref(),
    ) {
        Disposition::Refuse(why) => anyhow::bail!("signed in, but {why}"),
        Disposition::Switch(user_id) => {
            // Describe the account before selecting it. Seeding after the marker
            // moved would report a failed switch while the install already
            // pointed at the home that could not be written.
            let notice = seed_account_backend(&root.join(&user_id), base_url)?;
            // Only claim a switch the marker actually records. Swallowing this
            // would stop the app with "restart to finish signing in" while the
            // marker still names the previous account — the restart would return
            // there and the login would be silently lost. Either way nothing is
            // stored: this token is not this home's.
            medulla::home::user::write_active_user_id(&root, &user_id).map_err(|e| {
                anyhow::anyhow!(
                    "signed in as a different account, but this install could not be pointed at \
                     it ({e}) — the session was not stored"
                )
            })?;
            // The marker moved, so this now resolves the *new* account's home:
            // record the deployment that just identified them there, or their
            // first launch loads an empty home, falls back to production, and
            // cannot finish the login the restart was asked to complete.
            return Ok(SignIn::SwitchedAccount(notice));
        }
        Disposition::Store => {}
    }

    medulla::core_host::auth::store_session(core, jwt)
        .await
        .map_err(|e| anyhow::anyhow!("signed in, but the core rejected the session: {e}"))?;
    Ok(SignIn::SameAccount)
}

/// Whether this install has already been scoped to an account.
///
/// False means no `medulla login` has completed here (or the operator logged
/// out), so every path would resolve to the pre-login home.
pub(crate) fn account_is_active(env: &std::collections::HashMap<String, String>) -> bool {
    let root = medulla::home::medulla_root(env);
    medulla::home::user::active_user_id(env, &root) != medulla::home::user::PRE_LOGIN_USER_ID
}

/// Run the login screen for an install with no account yet, and record the
/// account it produces.
///
/// Returns the verified JWT — the caller hands it to the core once that core has
/// been booted against the now-known account's home — or `None` when the
/// operator quit the screen.
///
/// This owns a terminal session of its own, set up and torn down around the
/// screen, because the app's own guard cannot be installed yet: it outlives the
/// event loop, and the config that configures that loop is not resolved until
/// this returns.
///
/// # Errors
///
/// The terminal cannot be set up, the login flow fails, or the backend rejects
/// the token it just issued.
pub(crate) async fn sign_in_first_account(
    env: &std::collections::HashMap<String, String>,
    base_url: &str,
    alt_screen: bool,
) -> anyhow::Result<Option<String>> {
    let guard = TermGuard::setup(alt_screen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let outcome = run_login_screen(&mut terminal, base_url.to_string()).await;
    drop(guard);

    let jwt = match outcome? {
        LoginOutcome::Quit => return Ok(None),
        LoginOutcome::Token(jwt) => jwt,
    };

    // Ask the backend who this is. The core cannot answer it — its auth state
    // lives inside the directory this id chooses — and the token has to be
    // verified before it is trusted with a directory name either way.
    let me = medulla::client::MedullaClient::new(base_url.to_string(), jwt.clone())
        .me()
        .await
        .map_err(|e| anyhow::anyhow!("signed in, but the account could not be read: {e}"))?;
    // A refusal stops startup before the core is booted against a home that is
    // not this account's — the caller stores the token into it a moment later.
    crate::commands::adopt_account(env, &me, base_url)
        .map_err(|why| anyhow::anyhow!("signed in, but {why}"))?;
    Ok(Some(jwt))
}

/// Record, in the adopted account's own config, the deployment its session was
/// issued by — or say so when the account already names a different one.
///
/// Sessions are per-deployment, and until this ran nothing inside an account's
/// home said which one it belongs to: the `backend.baseUrl` that drove the login
/// came from the *pre-login* home, which the account home does not inherit. A
/// staging or self-hosted operator would sign in successfully and then have the
/// core bound to the production default, which rejects the token they just
/// minted.
///
/// Keyed on the setting, not the file: an account whose config holds a theme and
/// nothing else still has no deployment recorded.
///
/// # Why a differing value is reported rather than overwritten
///
/// `backend.baseUrl` decides which deployment the operator's agent talks to, and
/// the URL a login used can come from a deliberately transient source — an
/// `MEDULLA_API_URL` in one shell, a `--config` passed for one command. Silently
/// rewriting a configured endpoint from one of those repeats the mistake
/// `MEDULLA_USER` taught: a process-scoped choice must not quietly become the
/// persistent one. The mismatch does break the next launch, so it is named
/// loudly, with the file to edit, rather than either hidden or acted on.
///
/// Returns the mismatch notice for the caller to print, when the account names
/// a different deployment — rather than printing it here, because one caller
/// runs with the alt screen up and anything written there is discarded by the
/// restore that follows.
///
/// Takes the home explicitly rather than resolving it, so it can run *before*
/// the marker selects that account: a failure here must not leave the install
/// pointing at a home it could not write.
///
/// # Errors
///
/// The account's config cannot be created or written. The caller must not
/// complete a login on one: the account would be left pointing at a deployment
/// that never issued its session, which fails on the next launch with nothing to
/// explain it.
pub(crate) fn seed_account_backend(
    home: &std::path::Path,
    base_url: &str,
) -> anyhow::Result<Option<String>> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return Ok(None);
    }
    let path = home.join("config.toml");

    if let Some(existing) = declared_backend_base_url(&path) {
        if existing.trim_end_matches('/') == base_url.trim_end_matches('/') {
            return Ok(None);
        }
        // Returned, not printed. The account-switch path runs with the alt
        // screen up, so anything written to the terminal here is wiped by the
        // restore that follows — the caller knows when the screen is its own.
        return Ok(Some(format!(
            "Signed in to {base_url}, but this account's config names {existing}.\n\
             The next launch will use {existing} and this session will not work there — edit {} \
             or pass the same --config again.",
            path.display()
        )));
    }

    std::fs::create_dir_all(home)?;
    medulla::config::persist_setting(
        &path,
        "backend",
        "baseUrl",
        toml::Value::String(base_url.to_string()),
    )?;
    Ok(None)
}

/// The non-empty `backend.baseUrl` `path` already pins, if it pins one.
///
/// A file that cannot be read or parsed counts as pinning nothing: the write is
/// a merge that preserves every other key, so seeding over an unreadable file is
/// no worse than the state it is already in, and refusing would leave the
/// account undescribed.
fn declared_backend_base_url(path: &std::path::Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let doc = body.parse::<toml::Table>().ok()?;
    doc.get("backend")
        .and_then(toml::Value::as_table)
        .and_then(|backend| backend.get("baseUrl"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

/// What to do with a freshly verified token, given who it belongs to.
///
/// Split out as a pure function because these are the rules that decide whether
/// one account's bearer token can land in another account's credential store,
/// and every bug in this PR's review has been in them. Deciding without a
/// network call or a booted core means each rule is covered by a test rather
/// than by an integration path that is awkward to provoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// The token belongs to the account this process is running as: store it.
    Store,
    /// It belongs to somebody else, and this install may adopt them.
    Switch(String),
    /// It must not be stored here. Carries an operator-facing reason.
    Refuse(String),
}

/// Decide what may be done with a token authenticated as `authenticated`, by a
/// process running as `active`, with `overridden` telling whether that came from
/// `MEDULLA_USER` rather than the shared marker.
pub(crate) fn disposition(
    active: &str,
    overridden: bool,
    authenticated: Option<&str>,
) -> Disposition {
    // No id to compare. Storing anyway would defeat the check entirely: an
    // unverifiable token would be written into whatever account this process
    // happens to be running as.
    let Some(user_id) = authenticated else {
        return Disposition::Refuse(
            "the backend did not say which account this token belongs to — refusing to store a \
             session that may not be this account's"
                .to_string(),
        );
    };
    if user_id == active {
        return Disposition::Store;
    }
    // A switch joins this id to the root and writes there, so it is checked
    // before it can become a path — `..` or a separator would otherwise reach a
    // directory outside the account root. Not needed for the store case, which
    // joins nothing new: it is already running as that account.
    if medulla::home::user::sanitize_account_id(user_id).is_none() {
        return Disposition::Refuse(format!(
            "the backend named an account id that cannot be a directory name ({user_id:?})"
        ));
    }
    // The override is this process's own choice of account. Moving the shared
    // selection to satisfy it would change which account *every other* launch
    // opens, which is the opposite of what asking for one process was.
    if overridden {
        return Disposition::Refuse(format!(
            "signed in as {user_id}, but {} pins this process to {active} — nothing was stored, \
             and the shared account selection is unchanged",
            medulla::home::user::MEDULLA_USER_ENV,
        ));
    }
    Disposition::Switch(user_id.to_string())
}
