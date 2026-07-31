//! Non-TUI subcommand runners and the pre-app login screen driver.
//!
//! Holds the CLI verbs that do not enter the ratatui app — `medulla login`,
//! `logout`, `init`, and `workspace` — plus the credential persistence
//! helper and the interactive login-screen loop the TUI runs before selecting a
//! runtime. Each runner parses its own args, loads config, performs its work,
//! and returns an `anyhow::Result`.
//!
//! [`workspace`] and [`workflow`] own the registry and workflow verbs, which are
//! large enough to warrant their own files; everything else lives here.

pub(crate) mod login_screen;
#[cfg(feature = "workflows")]
pub(crate) mod workflow;
pub(crate) mod workspace;

pub(crate) use login_screen::run_login_screen;
#[cfg(feature = "workflows")]
pub(crate) use workflow::run_workflow_cmd;
pub(crate) use workspace::run_workspace;

use medulla::auth::{open_browser, run_login_flow, Credentials, LoopbackConfig};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla_tui::cli::{parse_init_args, parse_login_args, LoginArgs};

/// `medulla login`: obtain a JWT (loopback OAuth or a one-time token), verify it
/// with `/auth/me`, and hand it to the embedded core as its app session.
///
/// The core is the only credential store. This used to write a Medulla-owned
/// `credentials.json` beside it, which meant `medulla login` could report
/// success while the core — whose session actually drives the runtime — stayed
/// signed out, and the TUI would still open its login screen.
pub(crate) async fn run_login(args: &[String]) -> anyhow::Result<()> {
    let parsed: LoginArgs = match parse_login_args(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("medulla login: {msg}");
            std::process::exit(2);
        }
    };
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    let base_url = loaded.config.backend.base_url.clone();

    let jwt = match parsed.token {
        Some(token) => {
            // Headless fallback: redeem a one-time token, no listener.
            let client = MedullaClient::new(base_url.clone(), String::new());
            client
                .consume_login_token(token)
                .await
                .map_err(|e| anyhow::anyhow!("failed to redeem login token: {e}"))?
        }
        None => {
            let cfg = LoopbackConfig {
                no_browser: parsed.no_browser,
                ..Default::default()
            };
            run_login_flow(&base_url, parsed.provider, cfg, open_browser)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
        }
    };

    // Verify the token and greet the user.
    let client = MedullaClient::new(base_url.clone(), jwt.clone());
    let me = match client.me().await {
        Ok(me) => {
            println!("{}", medulla::auth::describe_me(&me));
            me
        }
        Err(e) => return Err(anyhow::anyhow!("token verification failed: {e}")),
    };

    // Record who this is *before* booting, because the id chooses the home the
    // core is about to be pointed at. Taken from `/auth/me` rather than from the
    // core's own auth state for the same reason: the core's state lives inside
    // the directory this id selects.
    //
    // A refusal stops the login here, before a core is booted or a token
    // written: the alternative is storing this account's bearer in a directory
    // that belongs to somebody else.
    adopt_account(&env, &me).map_err(|why| anyhow::anyhow!("signed in, but {why}"))?;

    // Carry the deployment this token was minted against into the account that
    // just became current, exactly as the TUI's first-run path does. `auth_core`
    // below reloads config from the *new* home; without this an account with no
    // config of its own falls back to the production default, and the core
    // rejects the JWT the configured deployment issued a moment ago.
    crate::sign_in::seed_account_backend(&env, &base_url);

    // Boot last: the flow above can take minutes of browser round-trip, and a
    // core sitting open across it buys nothing.
    let core = auth_core(&env).await?;
    medulla::core_host::auth::store_session(&core, &jwt)
        .await
        .map_err(|e| anyhow::anyhow!("the core rejected the session: {e}"))?;
    sweep_retired_credentials(&env);
    println!("Signed in to {base_url}.");
    Ok(())
}

/// Scope this install to the account named by an `/auth/me` response, and prove
/// that the home now in effect is that account's.
///
/// Writes the root-level active-user marker, which is what every later launch
/// reads to resolve its home — so this must happen before anything derives a
/// path from [`medulla::home::medulla_home`], and certainly before the core is
/// booted against one.
///
/// # Why this refuses rather than warns
///
/// A caller stores the session immediately after, into whichever home is in
/// effect. So "recorded the account" is not the question — "does the home this
/// process will use belong to the account that just authenticated" is, and three
/// things make those differ: a response with no id, a marker that cannot be
/// written, and `MEDULLA_USER`, which outranks the marker and therefore survives
/// a successful write. Any of them would put one account's bearer token in
/// another account's credential store. The comparison against
/// [`medulla::home::medulla_home`] covers all three at once, because it asks the
/// resolver the same question every later launch will ask it.
///
/// An id-less response is refused outright, including on a fresh install: the
/// pre-login home would accept it, but nothing would ever make it an *account*,
/// so every launch would sign in again and every such login would share one
/// credential store.
///
/// # Errors
///
/// Returns an operator-facing sentence when the effective home is not the
/// authenticated account's. The caller must not store a session after one.
pub(crate) fn adopt_account(
    env: &std::collections::HashMap<String, String>,
    me: &serde_json::Value,
) -> Result<(), String> {
    let root = medulla::home::medulla_root(env);

    let Some(user_id) = medulla::auth::user_id_from_me(me) else {
        // No carve-out for a fresh install. Letting an id-less login settle into
        // the pre-login home looks harmless — nothing to cross yet — but it
        // never becomes an account: `sign_in::account_is_active` keeps reading
        // `local` as signed out, so every launch runs the login flow again, and
        // every id-less account on the machine shares that one credential store.
        // The whole layout is keyed on this id, so a login without one has
        // nowhere correct to go and should say so.
        return Err(
            "the backend did not say which account this token belongs to, and every account's \
             directory is named by that id — there is nowhere correct to store this session"
                .to_string(),
        );
    };

    // `MEDULLA_USER` selects an account for *this process* without touching the
    // selection every other process reads, so honouring it means not writing the
    // marker at all — not even when the authenticated account matches, which
    // would still overwrite a marker naming somebody else. The verification
    // below is what makes that safe: an override pointing somewhere other than
    // the account that just authenticated fails instead of writing.
    let overridden = env
        .get(medulla::home::user::MEDULLA_USER_ENV)
        .map(|v| v.trim())
        .is_some_and(|v| !v.is_empty());
    if !overridden {
        medulla::home::user::write_active_user_id(&root, &user_id)
            .map_err(|err| format!("this account could not be recorded ({err})"))?;
    }

    let expected = root.join(&user_id);
    let effective = medulla::home::medulla_home(env);
    if effective != expected {
        return Err(format!(
            "signed in as {user_id}, but this process resolves its home to {} — unset \
             {} to use that account's own directory",
            effective.display(),
            medulla::home::user::MEDULLA_USER_ENV,
        ));
    }
    Ok(())
}

/// Boot the minimal core the auth verbs need, with its workspace bound.
///
/// Binding first is not optional: the core resolves its state directory from
/// `OPENHUMAN_WORKSPACE`, and an unbound run would write the session into the
/// developer's real `~/.openhuman` instead of the one this process's
/// `MEDULLA_HOME` implies — so `medulla login` would sign in a workspace the TUI
/// never reads.
async fn auth_core(
    env: &std::collections::HashMap<String, String>,
) -> anyhow::Result<medulla::core_host::EmbeddedCore> {
    let home = medulla::home::medulla_home(env);
    medulla::core_host::bind_workspace(env, &home);
    // The session must be stored against the backend this host is configured
    // for, or `medulla login` signs into one deployment and the TUI probes
    // another.
    let loaded = load_config(None, env, &home)?;
    medulla::core_host::bind_medulla_base_url(env, &loaded.config.backend.base_url);
    // And the core's own backend client with it: storing a session validates it
    // against `/auth/me` there, not on the Medulla base above.
    medulla::core_host::bind_backend_api_url(env, &loaded.config.backend.base_url);
    medulla::core_host::boot_for_auth()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start the embedded OpenHuman core: {e}"))
}

/// The core's app session as a `(base_url, jwt)` pair, or `None` when signed out.
///
/// `base_url` comes from the loaded Medulla config rather than the core: the two
/// address the same deployment by construction, and the core exposes no RPC for
/// the URL it resolved. A failure to reach the core reads as signed out — every
/// caller degrades to "no backend" rather than failing outright.
async fn session_credentials(
    env: &std::collections::HashMap<String, String>,
    base_url: &str,
) -> Option<Credentials> {
    let core = auth_core(env).await.ok()?;
    let jwt = medulla::core_host::auth::session_token(&core)
        .await
        .ok()??;
    Some(Credentials {
        base_url: base_url.to_string(),
        jwt,
    })
}

/// `medulla logout`: forget the core's app session.
///
/// Idempotent — logging out when already signed out succeeds, so a user who is
/// unsure of their state can always run it.
pub(crate) async fn run_logout() -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let core = auth_core(&env).await?;
    medulla::core_host::auth::clear_session(&core)
        .await
        .map_err(|e| anyhow::anyhow!("failed to clear the session: {e}"))?;
    sweep_retired_credentials(&env);
    // The session is cleared; the account selection is deliberately not.
    //
    // "Signed out" means "no session", not "no account". The account's directory
    // stays on disk either way, and the marker is what lets the next launch
    // *find* it — including its `config.toml`. Clearing the marker would send
    // that launch to the pre-login home and its default backend, so an operator
    // on staging or a self-hosted deployment would be offered a login against
    // production, with no way back to their own endpoint short of an environment
    // variable. The one thing logout must not do is make signing back in
    // impossible.
    //
    // So logging back in as the same account lands in the same home, with the
    // same config, logs, and workflow store. Signing in as somebody else moves
    // the marker through the account-switch path, which is the only thing that
    // should move it.
    println!("Logged out.");
    Ok(())
}

/// Delete any credential file left by a pre-cutover install, and say so.
///
/// Those files are no longer read, so nothing else would ever remove them —
/// and each one holds a real bearer token that no logout could invalidate.
fn sweep_retired_credentials(env: &std::collections::HashMap<String, String>) {
    let home = medulla::home::medulla_home(env);
    for path in medulla::auth::remove_retired_credentials(&home) {
        println!("Removed a retired credential file at {}.", path.display());
    }
}

/// `medulla hub`: run the orchestrator hub — bridge the hosted backend brain to
/// tiny.place worker daemons. Takes the backend JWT from the core's app session
/// and the worker roster from `MEDULLA_TINYPLACE_PEER` / `MEDULLA_HUB_WORKERS`.
pub(crate) async fn run_hub(_args: &[String]) -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    let loaded = load_config(None, &env, &home)?;
    let session = session_credentials(&env, &loaded.config.backend.base_url).await;
    // The standalone `medulla hub` owns its terminal, so stderr is right there.
    match crate::hub_relay::build_hub_config_with_log(
        &env,
        &home,
        medulla::hub::stderr_log(),
        session.as_ref(),
    ) {
        Some(config) => medulla::hub::run_hub(config).await,
        None => anyhow::bail!(
            "hub: nothing to run — set MEDULLA_TINYPLACE_PEER (or MEDULLA_HUB_WORKERS) and run \
             `medulla login` first"
        ),
    }
}

/// `medulla init [dir]` — author a `MEDULLA.md` workspace profile.
///
/// Reads the directory's `AGENTS.md` / `CLAUDE.md` / `README.md`, scans its file
/// layout, and writes an editable stub profile for the operator to fill in. The
/// model-drafted body went out with the memory layer that owned the provider
/// seam, so `--offline` is now the only behaviour there is.
///
/// This authors the file and stops there. `medulla workspace add` does the same
/// *and* enrols the directory in the registry, which is what the orchestrator
/// reads — see [`run_workspace`].
pub(crate) async fn run_init(args: &[String]) -> anyhow::Result<()> {
    let parsed = parse_init_args(args);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dir = parsed
        .dir
        .as_ref()
        .map_or_else(|| cwd.clone(), |d| cwd.join(d));

    let outcome = medulla::init::init_workspace(&dir, parsed.force).await?;
    workspace::report_profile(&outcome);
    println!(
        "Not registered — run `medulla workspace add` to let the orchestrator place work here."
    );
    Ok(())
}
