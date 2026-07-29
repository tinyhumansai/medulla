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
    match client.me().await {
        Ok(me) => println!("{}", medulla::auth::describe_me(&me)),
        Err(e) => return Err(anyhow::anyhow!("token verification failed: {e}")),
    }

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
