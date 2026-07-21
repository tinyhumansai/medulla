//! The `medulla daemon` CLI entry point: [`run_daemon`] wires provider
//! detection, identity/config bootstrap, tiny.place onboarding, and the
//! transport-backed serve loop around a [`DaemonRuntime`]. Flag parsing lives in
//! [`super::flags`]; the runtime state machine in [`super::runtime`] and
//! [`super::task_loop`].

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::onboarding::OnboardingUi;

use crate::tinyplace::{
    config_path, decode_task_frame, load_or_create_identity, resolve_endpoint,
    spawn_contact_auto_accepter, spawn_presence_heartbeat,
};
use ::tinyplace::api::directory::DirectoryApi;
use ::tinyplace::api::registry::RegisterRequest;
use ::tinyplace::types::AgentCard;
use ::tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};

use super::capabilities::read_git_facts;
use super::flags::{parse_provider, Flags};
use super::providers::{
    detect_providers, provider_bin, run_provider_task, RunTaskFn, RunTaskOptions, DAEMON_PROVIDERS,
};
use super::transport::{describe_error, SignalTransport};
use super::types::{
    DaemonConfig, DaemonRuntime, SendFn, DEFAULT_MAX_PENDING, DEFAULT_STATUS_THROTTLE_MS,
};

const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_TASK_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_POLL_MS: u64 = 2_000;
/// How often the serve loop runs (idempotent) key maintenance to refill a
/// consumed one-time pre-key pool. Long enough to be negligible overhead (the
/// call is a health-gated no-op when the pool is healthy).
const KEY_MAINTAIN_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(300);

/// Run `medulla daemon` until a shutdown signal. `args` are the tokens after
/// the `daemon` subcommand. `onboarding_ui` is the interactive first-run screen
/// the app injects on a TTY; pass `None` to onboard headlessly (scriptable).
pub async fn run_daemon(
    args: &[String],
    onboarding_ui: Option<OnboardingUi>,
) -> anyhow::Result<()> {
    let flags = Flags::parse(args).map_err(|e| anyhow::anyhow!(e))?;
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
    let handle = flags.string("handle");
    let extra_skills = flags.list("skills").unwrap_or_default();
    let once = flags.is_set("once");
    let reonboard = flags.is_set("reonboard");

    // First-run worker registration (naming + owner setup). With an injected
    // `onboarding_ui` this walks the operator through onboarding; without one it
    // auto-registers with defaults + an env owner so the daemon stays scriptable.
    // Aborting the interactive flow (q / Ctrl-C) exits cleanly without serving.
    let worker_profile =
        match crate::onboarding::ensure_registered(&env, reonboard, onboarding_ui).await? {
            Some(reg) => reg.profile,
            None => {
                log("onboarding aborted; not starting daemon");
                return Ok(());
            }
        };
    // The profile's name is the daemon's advertised label unless --name overrides it.
    let display_name = flags.string("name").or_else(|| {
        let name = worker_profile.name.trim();
        (!name.is_empty()).then(|| name.to_string())
    });

    // Identity + client.
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let config_file = config_path(&env, &home);
    let (signer, config) =
        load_or_create_identity(&config_file, &env).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let base_url = resolve_endpoint(&env, &config);
    let signer = Arc::new(signer);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.clone(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let identity_dir = config_file
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".tinyplace"));

    let transport = SignalTransport::new(client.clone(), &signer, &identity_dir);
    let agent_id = transport.agent_id().to_string();

    // Onboard (publish keys, register handle, upsert directory card) unless
    // suppressed. Key publishing is what lets peers open an encrypted channel.
    if !flags.is_set("no-onboard") {
        let git = read_git_facts(&workspace).await;
        let bio = format!(
            "Headless coding-agent daemon serving {} over tiny.place.{} cwd:{workspace}",
            providers
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            git.project
                .as_ref()
                .map(|p| format!(" project:{p}"))
                .unwrap_or_default(),
        );
        let mut skills: Vec<String> = std::iter::once("coding-agent".to_string())
            .chain(providers.iter().map(|p| p.as_str().to_string()))
            .chain(extra_skills.iter().cloned())
            .collect();
        dedupe(&mut skills);
        onboard(
            &transport,
            &signer,
            &client.directory,
            &agent_id,
            handle.as_deref(),
            display_name.as_deref(),
            &bio,
            &skills,
            &client,
            log,
        )
        .await;
        log(&format!(
            "onboarded {agent_id} (skills: {})",
            skills.join(", ")
        ));
    }

    // Runtime + transport-backed send.
    let send: SendFn = {
        let transport = transport.clone();
        Arc::new(move |to: String, body: String| {
            let transport = transport.clone();
            Box::pin(async move {
                if let Err(err) = transport.send(&to, &body).await {
                    eprintln!("medulla daemon: send to {to} failed: {err}");
                }
            })
        })
    };
    let config = DaemonConfig {
        providers: providers.clone(),
        default_provider,
        workspace: workspace.clone(),
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
    };
    let run_task: RunTaskFn =
        Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)));
    let runtime = DaemonRuntime::new(config, run_task, send)
        .with_log(Arc::new(|line: &str| eprintln!("medulla daemon: {line}")));

    // Contact auto-accept + presence run unlocked (pure REST, no ratchet).
    let accepter = spawn_contact_auto_accepter(
        client.clone(),
        std::time::Duration::from_millis(poll_ms),
        |_agent_id: &str| true,
    );
    let presence =
        spawn_presence_heartbeat(client.clone(), std::time::Duration::from_millis(poll_ms));

    if once {
        // Probe hook: accept pending contacts, drain the inbox once, wait for
        // every started task to settle, then exit.
        drain_once(&transport, &runtime).await;
        runtime.idle().await;
        accepter.abort();
        presence.abort();
        log("--once complete");
        return Ok(());
    }

    log(&format!(
        "serving providers [{}] as {agent_id} on {base_url} (workspace: {workspace})",
        providers
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Serve loop: poll → decode → dispatch, until a signal.
    let poll = tokio::time::Duration::from_millis(poll_ms);
    let mut sigterm = signal_stream()?;
    // Periodic key maintenance. As the responder, this worker's one-time pre-key
    // pool is consumed one key per new peer handshake; without a refill a
    // long-lived daemon eventually runs dry and can no longer complete X3DH.
    // `publish_keys` is health-gated (idempotent), so this is a no-op until the
    // relay pool actually runs low. The immediate first tick is consumed here
    // because we already published at startup.
    let mut maintain = tokio::time::interval(KEY_MAINTAIN_INTERVAL);
    maintain.tick().await;
    loop {
        tokio::select! {
            _ = &mut sigterm => {
                log("received shutdown signal, shutting down");
                break;
            }
            _ = maintain.tick() => {
                if let Err(e) = transport.publish_keys(&signer).await {
                    log(&format!("periodic key maintenance failed: {e}"));
                }
            }
            _ = tokio::time::sleep(poll) => {
                for message in transport.drain_inbox(50).await {
                    let frame = decode_task_frame(&message.text);
                    runtime.handle_message(message.from, message.text, frame);
                }
            }
        }
    }

    runtime.shutdown();
    accepter.abort();
    presence.abort();
    Ok(())
}

/// Accept pending contacts and dispatch one inbox drain (the `--once` probe path).
async fn drain_once(transport: &SignalTransport, runtime: &DaemonRuntime) {
    for message in transport.drain_inbox(50).await {
        let frame = crate::tinyplace::decode_task_frame(&message.text);
        runtime.handle_message(message.from, message.text, frame);
    }
}

/// Publish pre-keys, best-effort claim the handle, and upsert the directory card.
#[allow(clippy::too_many_arguments)]
async fn onboard(
    transport: &SignalTransport,
    signer: &LocalSigner,
    directory: &DirectoryApi,
    agent_id: &str,
    handle: Option<&str>,
    display_name: Option<&str>,
    bio: &str,
    skills: &[String],
    client: &TinyPlaceClient,
    log: impl Fn(&str),
) {
    // Publish Signal pre-keys (required for peers to message us).
    match transport.publish_keys(signer).await {
        Ok(()) => log("published Signal pre-keys"),
        Err(err) => log(&format!("pre-key publish failed: {err}")),
    }

    // Claim the handle (best-effort; needs funds).
    if let Some(handle) = handle {
        let result = client
            .registry
            .register(RegisterRequest {
                username: handle.to_string(),
                crypto_id: agent_id.to_string(),
                ..Default::default()
            })
            .await;
        match result {
            Ok(_) => log(&format!("registered handle {handle}")),
            Err(err) => log(&format!(
                "handle registration skipped: {}",
                describe_error(&err)
            )),
        }
    }

    // Upsert the directory card (best-effort). AgentCard has no Default, so
    // build it from JSON with only the fields we set (the rest default).
    let name = display_name
        .map(str::to_string)
        .or_else(|| handle.map(str::to_string))
        .unwrap_or_else(|| "coding-agent daemon".to_string());
    // `createdAt`/`updatedAt` are required by the card contract and always
    // serialized (they are plain `String`s, so a default empty value still goes
    // on the wire and the directory rejects it while parsing RFC 3339). Stamp
    // both with the SDK's own timestamp helper — the same one `messages.send`
    // uses — rather than letting them default.
    let now = ::tinyplace::auth::timestamp();
    // The directory checks that `publicKey` derives `cryptoId` and rejects the
    // card otherwise. They are two encodings of the same Ed25519 public key:
    // `cryptoId` is its base58 (the agent id), `publicKey` its base64.
    let public_key = ::tinyplace::crypto::public_key_to_base64(signer.public_key());
    let card: AgentCard = serde_json::from_value(serde_json::json!({
        "agentId": agent_id,
        "name": name,
        "description": bio,
        "username": handle,
        "cryptoId": agent_id,
        "publicKey": public_key,
        "skills": skills,
        "createdAt": now,
        "updatedAt": now,
    }))
    .expect("AgentCard JSON is well-formed");
    match directory.upsert_agent(agent_id, &card).await {
        Ok(_) => log("upserted directory card"),
        Err(err) => log(&format!(
            "directory upsert skipped: {}",
            describe_error(&err)
        )),
    }
}

/// Retain only the first occurrence of each value, preserving order.
fn dedupe(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
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
mod flag_tests {
    use super::*;
    // `Flags`/`parse_provider`/`dedupe` come from `super::*`; the provider enum
    // is only needed by these tests, so import it explicitly here.
    use crate::tinyplace::HarnessProvider;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_values_bools_and_lists() {
        let flags = Flags::parse(&args(&[
            "--workspace",
            "/tmp/x",
            "--providers",
            "claude,codex",
            "--providers",
            "opencode",
            "--once",
            "--dangerously-skip-permissions",
        ]))
        .unwrap();
        assert_eq!(flags.string("workspace").as_deref(), Some("/tmp/x"));
        assert!(flags.is_set("once"));
        assert!(flags.is_set("dangerously-skip-permissions"));
        assert!(!flags.is_set("no-onboard"));
        // Repeated + comma-joined lists flatten and trim.
        assert_eq!(
            flags.list("providers").unwrap(),
            vec!["claude", "codex", "opencode"]
        );
        // A later value wins for scalar lookups.
        let dup = Flags::parse(&args(&["--model", "a", "--model", "b"])).unwrap();
        assert_eq!(dup.string("model").as_deref(), Some("b"));
    }

    #[test]
    fn rejects_unknown_and_valueless_flags() {
        assert!(Flags::parse(&args(&["positional"])).is_err());
        match Flags::parse(&args(&["--model"])) {
            Err(err) => assert!(err.contains("needs a value"), "got: {err}"),
            Ok(_) => panic!("missing value should error"),
        }
    }

    #[test]
    fn number_and_positive_validation() {
        let flags = Flags::parse(&args(&[
            "--concurrency",
            "3",
            "--zero",
            "0",
            "--bad",
            "nope",
        ]))
        .unwrap();
        assert_eq!(flags.number("concurrency").unwrap(), Some(3));
        assert_eq!(flags.number("missing").unwrap(), None);
        assert!(flags.number("bad").is_err());
        assert_eq!(flags.positive("concurrency", 2).unwrap(), 3);
        assert_eq!(flags.positive("missing", 7).unwrap(), 7);
        assert!(flags.positive("zero", 2).is_err());
    }

    #[test]
    fn parse_provider_maps_wire_names() {
        assert_eq!(parse_provider("claude").unwrap(), HarnessProvider::Claude);
        assert_eq!(parse_provider("codex").unwrap(), HarnessProvider::Codex);
        let err = parse_provider("bogus").unwrap_err();
        assert!(err.contains("unknown provider"), "got: {err}");
    }

    #[test]
    fn dedupe_preserves_first_occurrence_order() {
        let mut values = args(&["a", "b", "a", "c", "b"]);
        dedupe(&mut values);
        assert_eq!(values, vec!["a", "b", "c"]);
    }
}
