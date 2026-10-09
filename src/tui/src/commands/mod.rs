//! Non-TUI subcommand runners and the pre-app login screen driver.
//!
//! Holds the CLI verbs that do not enter the ratatui app — `medulla login`,
//! `logout`, `init`, and `workspace` — plus the credential persistence
//! helper and the interactive login-screen loop the TUI runs before selecting a
//! runtime. Each runner parses its own args, loads config, performs its work,
//! and returns an `anyhow::Result`.
//!
//! [`workspace`], [`workflow`], and [`skills`] own the registry, workflow, and
//! harness-skill verbs, which are large enough to warrant their own files;
//! everything else lives here.

pub(crate) mod analytics_test;
pub(crate) mod hook;
pub(crate) mod login_screen;
#[cfg(feature = "workflows")]
pub(crate) mod mcp;
pub(crate) mod sentry_test;
#[cfg(feature = "workflows")]
pub(crate) mod skills;
#[cfg(feature = "workflows")]
pub(crate) mod workflow;
pub(crate) mod workspace;

pub(crate) use analytics_test::run_analytics_test;
pub(crate) use hook::run_hook_cmd;
pub(crate) use login_screen::run_login_screen;
#[cfg(feature = "workflows")]
pub(crate) use mcp::run_mcp_cmd;
pub(crate) use sentry_test::run_sentry_test;
#[cfg(feature = "workflows")]
pub(crate) use skills::run_skills_cmd;
#[cfg(feature = "workflows")]
pub(crate) use workflow::run_workflow_cmd;
pub(crate) use workspace::run_workspace;

use medulla::auth::{open_browser, run_login_flow, Credentials, LoopbackConfig};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla_tui::cli::{parse_init_args, parse_login_args, LoginArgs};

/// `medulla login`: obtain a JWT (loopback OAuth or a one-time token), verify it
/// with `/auth/me`, and store it as the app session via `medulla::auth::store`.
///
/// The stored session is the only credential store. An embedded core once sat
/// between this command and the session, which meant `medulla login` could
/// report success while the core — whose session actually drove the runtime —
/// stayed signed out, and the TUI would still open its login screen. With the
/// core gone, this command writes the one store every later launch reads.
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
        None if parsed.code => run_code_login(&base_url, parsed.provider).await?,
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

    // Record who this is before writing anything, because the id chooses the
    // home the session is about to be stored under. Taken from `/auth/me`
    // rather than from any existing local state, since that state lives inside
    // the directory this id selects.
    //
    // A refusal stops the login here, before a token is written: the
    // alternative is storing this account's bearer in a directory that
    // belongs to somebody else.
    // Before `adopt_account`, which moves the active-account marker. An inline
    // `backend.token` or a `backend.tokenEnv` variable outranks the store, so a
    // login under one writes a JWT nothing will present — and worse, re-homes
    // the install to the pasted token's account while every client keeps
    // authenticating as whoever the external token belongs to. The account on
    // screen and the identity on the wire would disagree.
    if let Some(source) =
        medulla::runtime::cloud::connect::external_credential_source(&env, &loaded.config.backend)
    {
        anyhow::bail!(
            "signed in, but {source} outranks the stored session — this shell would keep \
             using that credential, so the login was not saved; remove it and sign in again"
        );
    }

    adopt_account(&env, &me, &base_url).map_err(|why| anyhow::anyhow!("signed in, but {why}"))?;

    // Store last. `store` re-verifies the token against `/auth/me` before it
    // writes, which is the same contract the core's `auth_store_session` had —
    // the difference is that satisfying it no longer costs an OpenHuman core
    // boot on a flow that has already spent minutes on a browser round-trip.
    medulla::auth::store(&env, &base_url, &jwt)
        .await
        .map_err(|e| anyhow::anyhow!("could not store the session: {e}"))?;

    // After the store, because the store is what publishes the account marker
    // now. This asks the resolver the same question every later launch will —
    // if it disagrees, the session is on disk but the next process would look
    // somewhere else for it, and the operator should hear that from the command
    // that just claimed success.
    if let Some(account) = medulla::auth::user_id_from_me(&me) {
        let expected = medulla::home::medulla_root(&env).join(
            medulla::home::user::sanitize_account_id(&account).unwrap_or_else(|| account.clone()),
        );
        let effective = medulla::home::medulla_home(&env);
        if effective != expected {
            anyhow::bail!(
                "signed in as {account}, but this process resolves its home to {} — unset {} \
                 to use that account's own directory",
                effective.display(),
                medulla::home::user::MEDULLA_USER_ENV,
            );
        }
        // Verified by `/auth/me` and durably stored: later crash reports from
        // this process carry the account id (and nothing else about it).
        medulla::observability::set_user(Some(&account));
        // Best-effort, bounded by one shared deadline rather than the
        // tracker's own per-request timeout: `record_sign_in` makes two
        // sequential requests (identify, then the event), so awaiting each of
        // their individual three-second ceilings could delay this
        // already-completed login by nearly six seconds. `medulla login`
        // exits right after this, so the event has to be awaited somewhat —
        // a detached task would almost always be cancelled by the runtime
        // shutting down before it lands — but one request timeout is the
        // most a finished login should ever wait on it.
        let _ = tokio::time::timeout(
            medulla::analytics::REQUEST_TIMEOUT,
            medulla::analytics::record_sign_in(&account),
        )
        .await;
    }

    adopt_legacy_credentials(&env, &loaded.config.backend).await;
    println!("Signed in to {base_url}.");
    Ok(())
}

/// `medulla login --code`: the terminal sign-in, for a shell with no usable
/// browser of its own.
///
/// Prints a verification URL, waits for the operator to open it wherever they
/// can and paste back the one-time code that page shows, and exchanges that code
/// for a JWT. Nothing is bound locally — which is the point. Over SSH the
/// loopback flow does not merely inconvenience the operator, it cannot work:
/// the backend's redirect to `127.0.0.1` reaches the machine running the
/// browser, never the one running this command.
///
/// # Errors
///
/// Fails when stdin is closed or empty (nothing was pasted), or when the backend
/// refuses the code — expired, already used, or simply mistyped.
async fn run_code_login(
    base_url: &str,
    provider: medulla::auth::Provider,
) -> anyhow::Result<String> {
    use std::io::Write;

    let url = medulla::auth::code_login_url(base_url, provider);
    println!("Open this URL on any device and sign in:\n\n  {url}\n");
    print!("Then paste the code it shows here: ");
    std::io::stdout().flush().ok();

    // `read_line` blocks, and this runs on the same runtime that will serve the
    // redemption request, so it goes to a blocking thread rather than parking a
    // worker for however long the operator takes to switch devices.
    let code = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await??;

    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(anyhow::anyhow!("no code was pasted"));
    }

    let client = MedullaClient::new(base_url.to_string(), String::new());
    client
        .consume_login_token(code)
        .await
        .map_err(|e| anyhow::anyhow!("failed to redeem the login code: {e}"))
}

/// Scope this install to the account named by an `/auth/me` response, and prove
/// that the home now in effect is that account's.
///
/// Writes the root-level active-user marker, which is what every later launch
/// reads to resolve its home — so this must happen before anything derives a
/// path from [`medulla::home::medulla_home`], and certainly before the session
/// is stored under one.
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
    base_url: &str,
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

    // Before the id is joined to anything. It comes from a backend response and
    // becomes a directory name, so `..`, a separator, or an absolute path would
    // otherwise reach `seed_account_backend` — which creates directories and
    // merges a config file — at a path outside the root, on a login that may
    // then be refused anyway.
    let user_id = medulla::home::user::sanitize_account_id(&user_id).ok_or_else(|| {
        format!("the backend named an account id that cannot be a directory name ({user_id:?})")
    })?;

    // `MEDULLA_USER` selects an account for *this process* without touching the
    // selection every other process reads, so honouring it means not writing the
    // marker at all — not even when the authenticated account matches, which
    // would still overwrite a marker naming somebody else.
    let overridden = env
        .get(medulla::home::user::MEDULLA_USER_ENV)
        .map(|v| v.trim())
        .is_some_and(|v| !v.is_empty());

    let expected = root.join(&user_id);
    // `root` is otherwise unused now that the marker write has moved to
    // `auth::store`; `expected` is what the checks below and the seed need.
    let _ = &root;

    // The override is checked before anything is written. A refused login must
    // leave no trace: seeding first would create the authenticated account's
    // config in a home this process is not going to use, on a login that ends
    // in an error.
    if overridden && medulla::home::medulla_home(env) != expected {
        return Err(format!(
            "signed in as {user_id}, but {} pins this process to another account — unset it to \
             use that account's own directory",
            medulla::home::user::MEDULLA_USER_ENV,
        ));
    }

    // Describe the account without selecting it. Seeding is safe to do early —
    // it writes into the authenticated account's own directory — but the
    // *marker* is not: publishing it here and storing the session afterwards
    // means a failed store leaves the previous account's valid session hidden
    // behind a marker naming an account that has none.
    //
    // So the marker write lives in `auth::store`, which publishes it only once
    // the session it points at is durable. This function's job ends at "this
    // account is safe to adopt, and its config exists".
    let notice = crate::sign_in::seed_account_backend(&expected, base_url)
        .map_err(|err| format!("this account's config could not be written ({err})"))?;
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    Ok(())
}

/// The stored app session as a `(base_url, jwt)` pair, or `None` when signed out.
///
/// The URL comes from the loaded config rather than from the stored record: the
/// record's own `baseUrl` is diagnostic, and the config is what the rest of this
/// process addresses. Nothing resolvable reads as signed out — every caller
/// degrades to "no backend" rather than failing outright.
fn session_credentials(
    env: &std::collections::HashMap<String, String>,
    backend: &medulla::config::BackendConfig,
) -> Option<Credentials> {
    // The whole precedence chain, not the store alone. `medulla hub` reads a
    // bearer the same way every other backend surface does, or an operator
    // running on `MEDULLA_TOKEN` is told to `medulla login` while that token is
    // already authenticating the TUI and the runtime beside it.
    let session = medulla::auth::session_token(env);
    let jwt = medulla::auth::resolve_backend_token(env, backend, session.as_deref())?;
    Some(Credentials {
        base_url: backend.base_url.clone(),
        jwt,
    })
}

/// `medulla logout`: forget the stored app session.
///
/// Idempotent — logging out when already signed out succeeds, so a user who is
/// unsure of their state can always run it.
pub(crate) async fn run_logout() -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Read the account before deleting the session that names it, but do not
    // report or clear anything yet: a `signed_out` event and a cleared crash
    // report user for a logout that then fails to clear (a read-only mount, a
    // permissions change, a file another process still holds open) would lie
    // about the session's real state — the bearer stays active while every
    // signal says otherwise.
    //
    // The account is the one reports are attributed to, which startup has
    // already held to every trust rule (an external credential, a session a
    // cwd `.env` re-selected), as the TUI's sign-out paths do; re-reading the
    // session from the environment would let a checkout's `.env` name a
    // planted account in the `signed_out` event.
    let user_id = medulla::observability::reported_user();
    medulla::auth::clear(&env)
        .map_err(|e| anyhow::anyhow!("the stored session could not be removed: {e}"))?;
    let external_credential_remains = load_config(None, &env, &cwd).ok().is_some_and(|loaded| {
        medulla::auth::resolve_backend_token(&env, &loaded.config.backend, None).is_some()
    });
    // Only now is the sign-out real: report it (bounded by the tracker's
    // timeout, since this process exits right after) and clear the crash
    // report user, so no later report is attributed to the retired session.
    // An analytics failure never blocks the sign-out itself.
    if !external_credential_remains {
        if let Some(user_id) = user_id {
            let _ = tokio::time::timeout(
                medulla::analytics::REQUEST_TIMEOUT,
                medulla::analytics::record_sign_out(&user_id),
            )
            .await;
        }
    }
    medulla::observability::set_user(None);

    // Retired credential files go too. Adoption is a *startup* behaviour — an
    // install that predates the store is signed in and should stay signed in —
    // but that is exactly what makes leaving one here wrong: the next launch
    // would verify the retired bearer and sign the operator straight back in,
    // having just been told they were logged out. Logging out is the one moment
    // those files must not survive.
    for legacy in medulla::auth::adopt_legacy_credentials(&env, &home) {
        if medulla::auth::discard_legacy_credential(&legacy.path) {
            println!(
                "Removed a retired credential file at {}.",
                legacy.path.display()
            );
        }
    }

    // Same check the runtime's own logout makes, for the same reason: an inline
    // `backend.token` or a `backend.tokenEnv` variable outranks the store, so
    // clearing it leaves the next process authenticated. Reported after the
    // removals, which should happen either way — the session really is gone, and
    // what remains is a source this command cannot reach.
    if let Ok(loaded) = load_config(None, &env, &cwd) {
        if medulla::auth::resolve_backend_token(&env, &loaded.config.backend, None).is_some() {
            let source = if loaded.config.backend.token.is_some() {
                "`backend.token` in the config".to_string()
            } else {
                format!(
                    "the `{}` environment variable",
                    loaded.config.backend.token_env
                )
            };
            anyhow::bail!(
                "the stored session was cleared, but this shell is still authenticated \
                 from {source} — remove it to finish signing out"
            );
        }
    }
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

/// Adopt any credential a pre-cutover install left behind, and say so.
async fn adopt_legacy_credentials(
    env: &std::collections::HashMap<String, String>,
    backend: &medulla::config::BackendConfig,
) {
    for path in medulla::auth::adopt_legacy(env, backend).await {
        println!("Removed a retired credential file at {}.", path.display());
    }
}

/// `medulla hub`: run the orchestrator hub — bridge the hosted backend brain to
/// host-link worker daemons. Takes the backend JWT from the stored app session
/// and the worker roster from `MEDULLA_LINK_PEER` / `MEDULLA_HUB_WORKERS`.
pub(crate) async fn run_hub(_args: &[String]) -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    let loaded = load_config(None, &env, &home)?;
    let session = session_credentials(&env, &loaded.config.backend);
    // The standalone `medulla hub` owns its terminal, so stderr is right there.
    match crate::hub_relay::build_hub_config_with_log(
        &env,
        &home,
        medulla::hub::stderr_log(),
        session.as_ref(),
    ) {
        Some(config) => medulla::hub::run_hub(config).await,
        None => anyhow::bail!(
            "hub: nothing to run — set MEDULLA_LINK_PEER (or MEDULLA_HUB_WORKERS) and run \
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
