//! The `medulla daemon` CLI entry point: [`run_daemon`] wires provider
//! detection, worker onboarding, the host link, and the link-backed serve loop
//! around a [`DaemonRuntime`]. Flag parsing lives in [`super::flags`]; the
//! runtime state machine in [`super::runtime`] and [`super::task_loop`].

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::onboarding::OnboardingUi;

use medulla_link::{Link, LinkConfig};

use crate::bridge::{Bridge as _, LinkBridge};
use crate::protocol::decode_task_frame;

use super::flags::{parse_provider, Flags};
use super::providers::{
    detect_providers, provider_bin, run_provider_task, RunTaskFn, RunTaskOptions, DAEMON_PROVIDERS,
};
use super::types::{
    DaemonConfig, DaemonRuntime, SendFn, DEFAULT_MAX_PENDING, DEFAULT_STATUS_THROTTLE_MS,
};

/// Host-wide task slots, effectively unlimited by default.
///
/// This cap predates declared agents: when a machine was one worker with one
/// implicit session, a small number was the only thing standing between a
/// fan-out and a thrashed laptop. It is the wrong instrument now, and it queued
/// invisibly — a third concurrent task waited on a slot even when it targeted a
/// different agent in a different workspace, where nothing could collide.
///
/// What actually bounds concurrency now sits at the grain that owns the hazard:
/// per-agent `max_sessions` derived from the agent's workspace strategy, and the
/// checkout serialization that keeps a second writer out of a tree someone is
/// already working in. A host-wide count knows about neither, so it can only
/// delay work that was already safe.
///
/// The semaphore itself stays — `active_count` derives the running-task figure
/// from its permits — and an operator can still set `concurrency` in config to
/// impose a real cap on a small machine.
const DEFAULT_CONCURRENCY: usize = 1024;
const DEFAULT_TASK_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_POLL_MS: u64 = 2_000;

/// Run `medulla daemon` until a shutdown signal. `args` are the tokens after
/// the `daemon` subcommand. `onboarding_ui` is the interactive first-run screen
/// the app injects on a TTY; pass `None` to onboard headlessly (scriptable).
pub async fn run_daemon(
    args: &[String],
    onboarding_ui: Option<OnboardingUi>,
) -> anyhow::Result<()> {
    let flags = Flags::parse(args).map_err(|e| anyhow::anyhow!(e))?;
    // Recorded before anything spawns a subprocess: a task this daemon runs
    // over ACP hands the harness an MCP server served by another invocation of
    // this same binary (`medulla workflow mcp`, see `daemon::providers::acp`),
    // and that subprocess only sees environment, not this process's parsed
    // flags. Without this, `medulla daemon --config foo.toml` would still have
    // its harness sessions' workflow tools re-discover a different config from
    // their own `cwd`. See `crate::config::CONFIG_PATH_ENV`.
    if let Some(path) = flags.string("config") {
        std::env::set_var(crate::config::CONFIG_PATH_ENV, path);
    }
    let env: HashMap<String, String> = std::env::vars().collect();
    let log = |line: &str| eprintln!("medulla daemon: {line}");

    // Provider detection.
    let only = match flags.list("providers") {
        Some(raw) => Some(
            raw.iter()
                .map(|entry| parse_provider(entry))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => None,
    };
    let providers = detect_providers(&env, only.as_deref(), None);
    if providers.is_empty() {
        let wanted = only
            .as_deref()
            .unwrap_or(&DAEMON_PROVIDERS)
            .iter()
            .map(|p| format!("{} ({})", p.as_str(), provider_bin(*p, &env)))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "no coding-agent CLI found on PATH — looked for: {wanted}. Install one or pass --providers."
        );
    }

    let default_provider = match flags.string("default-provider") {
        Some(requested) => {
            let provider = parse_provider(&requested).map_err(|e| anyhow::anyhow!(e))?;
            if !providers.contains(&provider) {
                anyhow::bail!(
                    "--default-provider \"{requested}\" is not available; detected: {}",
                    providers
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            provider
        }
        None => providers[0],
    };

    let workspace = flags
        .string("workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let workspace = std::fs::canonicalize(&workspace)
        .unwrap_or(workspace)
        .to_string_lossy()
        .into_owned();
    let concurrency = flags
        .positive("concurrency", DEFAULT_CONCURRENCY as u64)
        .map_err(|e| anyhow::anyhow!(e))? as usize;
    let task_timeout_ms = flags
        .positive("task-timeout-ms", DEFAULT_TASK_TIMEOUT_MS)
        .map_err(|e| anyhow::anyhow!(e))?;
    let poll_ms = flags
        .positive("poll-ms", DEFAULT_POLL_MS)
        .map_err(|e| anyhow::anyhow!(e))?;
    let max_pending = flags
        .positive("max-pending", DEFAULT_MAX_PENDING as u64)
        .map_err(|e| anyhow::anyhow!(e))? as usize;
    let status_throttle_ms = flags
        .number("status-throttle-ms")
        .map_err(|e| anyhow::anyhow!(e))?
        .map(|v| v as i64)
        .unwrap_or(DEFAULT_STATUS_THROTTLE_MS);
    let model = flags.string("model");
    let opencode_agent = flags.string("opencode-agent");
    let skip_permissions = flags.is_set("dangerously-skip-permissions");
    let once = flags.is_set("once");
    let reonboard = flags.is_set("reonboard");

    // First-run worker registration (naming + owner setup). With an injected
    // `onboarding_ui` this walks the operator through onboarding; without one it
    // auto-registers with defaults + an env owner so the daemon stays scriptable.
    // Aborting the interactive flow (q / Ctrl-C) exits cleanly without serving.
    let worker_profile = match crate::onboarding::ensure_registered_in(
        &env,
        reonboard,
        onboarding_ui,
        std::path::Path::new(&workspace),
    )
    .await?
    {
        Some(reg) => reg.profile,
        None => {
            log("onboarding aborted; not starting daemon");
            return Ok(());
        }
    };
    // The profile's name is the daemon's advertised label unless --name overrides it.
    let _display_name = flags.string("name").or_else(|| {
        let name = worker_profile.name.trim();
        (!name.is_empty()).then(|| name.to_string())
    });

    // The host link.
    //
    // The identity is *acquired*, not merely loaded: the daemon takes an
    // exclusive OS lock on `<medulla home>/link` and holds it for its whole
    // life. Without that a second daemon on this machine would draw sequences
    // from a second counter under the same pair key — which reuses AEAD nonces,
    // and is a confidentiality break rather than a race to tidy up later
    // (protocol §3.1). The lock is released when the link driver stops, which is
    // when the handle below is dropped: the end of `run_daemon`.
    let home = crate::home::medulla_home(&env);
    let explicit_config = flags.string("config");
    if let Some(path) = explicit_config.as_deref() {
        if !std::path::Path::new(path).is_file() {
            anyhow::bail!("explicit daemon configuration does not exist: {path}");
        }
    }
    let loaded_link_config = crate::config::load_config(
        explicit_config.as_deref(),
        &env,
        std::path::Path::new(&workspace),
    );
    let link_dir = match loaded_link_config {
        Ok(loaded) => loaded
            .config
            .link
            .map(|link| PathBuf::from(link.state_dir))
            .unwrap_or_else(|| medulla_link::keys::link_dir(&home)),
        Err(err) if explicit_config.is_some() => {
            anyhow::bail!("explicit daemon configuration failed to load: {err}")
        }
        Err(_) => medulla_link::keys::link_dir(&home),
    };
    // The orchestrator this host serves. A host enrolls against exactly one
    // (protocol §7.3), so this is also the link's single peer.
    let owner = crate::onboarding::env_owner(&env)
        .or_else(|| {
            let owner = worker_profile.owner.clone()?;
            (!owner.trim().is_empty()).then_some(owner)
        })
        .unwrap_or_else(|| "orchestrator".to_string());
    let node_name = if !worker_profile.address.trim().is_empty() {
        worker_profile.address.trim().to_string()
    } else {
        worker_profile.name.clone()
    };
    let link = Link::connect(LinkConfig::new(&link_dir))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "could not bring up the host link at {} ({e}) — enroll this machine first",
                link_dir.display()
            )
        })?;
    let transport = LinkBridge::single_peer(Arc::new(link), node_name, owner.clone())
        .map_err(|e| anyhow::anyhow!("host link is misconfigured: {e}"))?;
    let agent_id = transport.address().to_string();

    // Pairing hand-off. Adding this worker to an orchestrator needs exactly one
    // string to travel — this address — and the machine printing it is usually
    // one the operator reached over SSH, where selecting it by hand is the step
    // that makes people give up. So it goes to their terminal's clipboard and
    // onto a line of its own. `--no-pair` opts out for scripted runs that parse
    // this output. See [`super::pairing`].
    if !flags.is_set("no-pair") {
        let stdout_is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let handoff = super::pairing::clipboard_handoff(&agent_id, stdout_is_terminal);
        if let Some(escape) = &handoff {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(escape.as_bytes());
            let _ = out.flush();
        }
        eprint!(
            "{}",
            super::pairing::pairing_banner(&agent_id, None, handoff.is_some())
        );
    }

    // Runtime + link-backed send.
    let send: SendFn = {
        let transport = transport.clone();
        Arc::new(move |to: String, body: String| {
            let transport = transport.clone();
            Box::pin(async move {
                loop {
                    match transport.send(&to, &body).await {
                        Ok(()) => break,
                        Err(err) if err.starts_with("link queue overflow:") => {
                            // QueueOverflow is explicitly retryable: retain the
                            // frame until peer acknowledgements free history.
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                        Err(err) => {
                            eprintln!("medulla daemon: send to {to} failed: {err}");
                            break;
                        }
                    }
                }
            })
        })
    };
    // The custom OpenAI-compatible router and operator budget config, both read
    // from the same layered config the TUI loads (`--config` override, else
    // project-local/user-global discovery). A malformed or missing config leaves
    // both off rather than failing the daemon — routing is an optional overlay,
    // not a boot dependency — but the failure is narrated so "my [router] does
    // nothing" is answerable even when `--config` was passed explicitly.
    let loaded_config = match crate::config::load_config(
        flags.string("config").as_deref(),
        &env,
        std::path::Path::new(&workspace),
    ) {
        Ok(loaded) => Some(loaded),
        Err(err) => {
            log(&format!(
                "config load failed ({err}); routing and budgets are off"
            ));
            None
        }
    };
    let router = loaded_config
        .as_ref()
        .and_then(|loaded| loaded.config.router.clone());
    let budget = loaded_config
        .as_ref()
        .and_then(|loaded| loaded.config.budget.clone());
    // A config that failed to load must not silently drop attribution, so fall
    // back to the config type's own default (on).
    let attribution = loaded_config
        .as_ref()
        .map(|loaded| loaded.config.attribution.commit)
        .unwrap_or(true);
    // A config that failed to load declares no hooks.
    let hooks = loaded_config
        .as_ref()
        .map(|loaded| loaded.config.hooks.clone())
        .unwrap_or_default();
    let custom_harnesses = loaded_config
        .as_ref()
        .map(|loaded| {
            crate::config::load_layered_custom_harnesses(&loaded.sources).unwrap_or_else(|error| {
                log(&format!("custom harness config load failed ({error})"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    let config = DaemonConfig {
        hooks,
        providers: providers.clone(),
        default_provider,
        workspace: workspace.clone(),
        accessible_dirs: vec![workspace.clone()],
        env: env.clone(),
        task_timeout_ms,
        capability_timeout_ms: None,
        concurrency,
        status_throttle_ms,
        max_pending,
        model,
        agent: opencode_agent,
        extra_args: Vec::new(),
        skip_permissions,
        // The custom OpenAI-compatible router from the layered `[router]` config,
        // layered into every task's spawn environment by the executor.
        router,
        attribution,
        custom_harnesses,
        // Operator-declared per-provider budgets from the `[budget]` config,
        // advertised on the capability probe as `source: configured`.
        budget,
    };
    let run_task: RunTaskFn =
        Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)));
    let runtime = DaemonRuntime::new(config, run_task, send)
        .with_log(Arc::new(|line: &str| eprintln!("medulla daemon: {line}")));

    if once {
        // Probe hook: drain the inbox once, wait for every started task to
        // settle, then exit.
        transport
            .wait_for_inbox(tokio::time::Duration::from_millis(poll_ms))
            .await;
        drain_once(&transport, &runtime).await;
        runtime.idle().await;
        log("--once complete");
        return Ok(());
    }

    log(&format!(
        "serving providers [{}] as {agent_id} for {owner} (workspace: {workspace})",
        providers
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Serve loop: poll → decode → dispatch, until a signal.
    let poll = tokio::time::Duration::from_millis(poll_ms);
    let mut sigterm = signal_stream()?;
    loop {
        tokio::select! {
            _ = &mut sigterm => {
                log("received shutdown signal, shutting down");
                break;
            }
            // Returns early when the link's pump delivers, so a task frame is
            // picked up at about a round trip rather than up to a poll interval
            // later. The interval stays the correctness floor.
            _ = transport.wait_for_inbox(poll) => {
                for message in transport.drain_inbox(50).await {
                    let frame = decode_task_frame(&message.text);
                    // Every peer here is remote — the standalone daemon serves
                    // only over the host link — so a frame's `workflowNode`
                    // marker can never buy workflow authority. The transport
                    // states the verdict rather than hard-coding `false` here so
                    // a future device-local transport needs no reminder.
                    let sender_device_local = transport.is_device_local(&message.from).await;
                    runtime.handle_message_from(message.from, message.text, frame, sender_device_local);
                }
            }
        }
    }

    runtime.shutdown();
    Ok(())
}

/// Dispatch one inbox drain (the `--once` probe path).
async fn drain_once(transport: &LinkBridge, runtime: &DaemonRuntime) {
    for message in transport.drain_inbox(50).await {
        let frame = crate::protocol::decode_task_frame(&message.text);
        runtime.handle_message(message.from, message.text, frame);
    }
}

/// A future that resolves on SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
fn signal_stream() -> anyhow::Result<Pin<Box<dyn Future<Output = ()> + Send>>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        Ok(Box::pin(async move {
            tokio::select! {
                _ = sigint.recv() => {}
                _ = sigterm.recv() => {}
            }
        }))
    }
    #[cfg(not(unix))]
    {
        Ok(Box::pin(async move {
            let _ = tokio::signal::ctrl_c().await;
        }))
    }
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod flag_tests;
