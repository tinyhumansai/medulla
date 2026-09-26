//! `medulla daemon --direct`: bring a remote host up and serve one client.
//!
//! The whole sequence, in order, because each step depends on the one before:
//!
//! 1. Read `--peer-node`, the client's node id. It is public — node ids ride in
//!    cleartext headers by design — so it is the one half of the pairing that
//!    can safely travel in argv.
//! 2. Mint or open this host's identity and enroll that client under a fresh
//!    pair key ([`enroll_client`]).
//! 3. Bind a UDP socket and bring up a [`LinkPath::Direct`] link.
//! 4. Print the connect line to stdout, where the client's SSH session reads it,
//!    and flush — SSH may close the moment it has the line.
//! 5. Serve until told otherwise.
//!
//! Nothing here contacts the backend, and nothing requires the host to have been
//! enrolled or to have a coding CLI installed. A machine with `/bin/sh` and this
//! binary is a usable remote host, which is the point.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use medulla::bridge::{LinkBridge, LinkBridgeConfig, LinkPeer};
use medulla_link::keys::NodeId;
use medulla_link::{Link, LinkConfig, LinkPath, PeerConfig};

use super::{
    bind_address, client_dir, control, enroll_client, link_dir, running_instance, write_instance,
    ConnectLine, Instance, RemoteServer,
};
use crate::ui::harness_pane::LocalSessions;
use crate::worker::pty::PtyManager;

/// The bridge address the client is known by on this side.
///
/// A constant because a direct link has exactly one peer: there is nobody else
/// to tell apart, and a name derived from the node id would only make log lines
/// harder to read.
pub const CLIENT_ADDRESS: &str = "medulla-client";

/// Run the direct-serving daemon.
///
/// # Errors
///
/// When `--peer-node` is missing or unparseable, when the identity cannot be
/// minted or locked, or when the UDP socket cannot be bound. Each is fatal:
/// there is no degraded mode where a client could still reach this process.
pub async fn run(args: &[String]) -> anyhow::Result<()> {
    let peer = flag(args, "--peer-node")
        .and_then(|value| NodeId::from_hex(&value))
        .ok_or_else(|| {
            anyhow::anyhow!("--peer-node <32 hex> is required; the client supplies its node id")
        })?;
    let port: u16 = flag(args, "--port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    let env: HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    // The far side's own config, not the client's. A remote host's `[router]`,
    // `[[hooks]]`, `[attribution]` and `[[customHarnesses]]` describe *that*
    // machine — its API keys, its hook commands, its presets — and shipping the
    // client's over would be both wrong and a way to make one machine run
    // another's commands.
    let explicit_config = flag(args, "--config");
    if let Some(path) = explicit_config.as_deref() {
        std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
    }
    let cwd = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    // Where sessions this host serves will run. The single most consequential
    // argument, exactly as it is for `daemon --tui`: a harness serving a client
    // edits files here, so defaulting to the launch directory means the shell
    // that started the daemon decides what the client can touch.
    let workspace = flag(args, "--workspace")
        .map(|dir| {
            std::fs::canonicalize(&dir)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or(dir)
        })
        .unwrap_or_else(|| cwd.clone());

    let loaded =
        medulla::config::load_config(explicit_config.as_deref(), &env, std::path::Path::new(&cwd))?;

    let dir = link_dir(&client_dir(&home, peer));
    // Held across enrollment *and* link startup. `enroll_client` releases
    // `node.lock` so `Link::connect` can take it, and a second bootstrap
    // arriving in that gap would overwrite the key this daemon is about to run
    // with. See `BootstrapLock`.
    let bootstrap_hold = super::hold_bootstrap(&client_dir(&home, peer))?;
    let (node_id, pair_key) = match enroll_client(&dir, peer) {
        Ok(pair) => pair,
        // The lock is held, so a daemon for this client is already running — and
        // it owns that client's sessions. Hand back its door rather than
        // starting a second daemon the client could reach but that has none of
        // its sessions in it.
        Err(medulla_link::keys::KeyError::Busy(path)) => match running_instance(&home, peer) {
            Some(line) => {
                println!("{}", line.render());
                std::io::stdout().flush()?;
                return Ok(());
            }
            None => anyhow::bail!(
                "a daemon holds {} but published no usable instance;                  stop it and try again",
                path.display()
            ),
        },
        Err(error) => return Err(error.into()),
    };

    let mut config = LinkConfig::new(&dir);
    config.bind = bind_address(port);
    // The client's address is not known yet and does not need to be: it dials in,
    // and the link learns where it is from the first datagram that authenticates
    // (see `medulla_link::link::direct`). An unroutable placeholder is honest
    // about that — nothing is ever sent there, because nothing is sent until the
    // client has been heard from.
    config.path = LinkPath::Direct {
        endpoint: super::unrouted_placeholder(config.bind),
    };
    config.peers = vec![PeerConfig {
        node_id: peer,
        pair_key: pair_key.clone(),
    }];
    let link = Link::connect(config).await?;
    let bound = link.local_addr();

    let bridge = Arc::new(LinkBridge::new(
        Arc::new(link),
        LinkBridgeConfig {
            node_name: "medulla-host".to_string(),
            peers: vec![LinkPeer {
                name: CLIENT_ADDRESS.to_string(),
                node_id: peer,
            }],
        },
    ));

    // Bound before the first session can be opened: `attach_mcp` reads the
    // process-wide plane at launch, so a session started before this exists
    // would silently get no tools and nothing later would give it any. Held for
    // the life of the daemon — dropping it unbinds the socket and makes every
    // grant minted against it unredeemable.
    let _control = control::start(&env, &loaded.config, &client_dir(&home, peer), |line| {
        eprintln!("{line}")
    })
    .await;

    // Presets live in their own layered files rather than on the document, so
    // they are resolved from the same sources the config came from — which is
    // what makes a `[[customHarnesses]]` entry defined on the *host* offerable
    // to a client that has never heard of it.
    let presets =
        medulla::config::load_layered_custom_harnesses(&loaded.sources).unwrap_or_default();
    let sessions = local_sessions(&env, &loaded.config, &presets, &workspace);
    let host_name = flag(args, "--host-name")
        .unwrap_or_else(|| hostname_of(&env).unwrap_or_else(|| "remote host".to_string()));
    let workspaces = flag(args, "--workspaces")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|dir| !dir.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // Published before the line goes out, so a bootstrap racing this one finds a
    // complete instance rather than a half-written one.
    write_instance(
        &home,
        peer,
        &Instance {
            port: bound.port(),
            node_id,
        },
    )?;
    // Released here rather than as soon as the link was up. A second bootstrap
    // freed in that gap sees `node.lock` busy — this daemon holds it — but finds
    // no instance file yet, or a stale one from an earlier daemon, so it fails
    // spuriously or hands its client the wrong port. The window the lock has to
    // cover is enroll → connect → *publish*, because publication is what makes
    // the reuse path answerable.
    //
    // Still released before serving: holding it for the daemon's life would make
    // the next bootstrap wait for this one to exit rather than reuse it.
    drop(bootstrap_hold);

    // Printed and flushed before serving: the client is blocked on this line, and
    // SSH may close the moment it has it.
    let line = ConnectLine {
        port: bound.port(),
        node_id,
        pair_key,
    };
    println!("{}", line.render());
    std::io::stdout().flush()?;

    // Only now, with the line safely on its way: this closes stdout, so doing it
    // any earlier would swallow the very thing the client is waiting for.
    // Reported and continued on failure — a daemon tied to its SSH session still
    // serves, and refusing to start would be strictly worse.
    if let Err(error) = super::detach_from_ssh() {
        eprintln!(
            "remote: could not detach from the SSH session ({error}); \
                   sessions will end when it does"
        );
    }

    let mut server = RemoteServer::new(sessions, bridge, host_name).with_workspaces(workspaces);
    server.run().await;
    Ok(())
}

/// Build the session factory this host serves with.
///
/// Deliberately the same `LocalSessions` the operator's own picker uses, so a
/// session opened from across the network is launched by exactly the code that
/// launches one opened here — MCP registration, managed skills, router injection
/// and attribution included for a coding agent, and deliberately none of them
/// for a shell.
pub(super) fn local_sessions(
    env: &HashMap<String, String>,
    config: &medulla::config::TuiConfig,
    presets: &[medulla::config::CustomHarnessConfig],
    workspace: &str,
) -> LocalSessions {
    // `None` for the filter: a remote host offers whatever it actually has,
    // rather than a list configured on the client's machine.
    let providers = medulla::daemon::providers::detect_providers(env, None, None);
    let daemon = medulla::daemon::DaemonConfig {
        hooks: config.hooks.clone(),
        providers: providers.clone(),
        default_provider: providers
            .first()
            .copied()
            .unwrap_or(medulla::protocol::HarnessProvider::Shell),
        workspace: workspace.to_string(),
        accessible_dirs: Vec::new(),
        env: env.clone(),
        task_timeout_ms: 600_000,
        capability_timeout_ms: None,
        concurrency: 4,
        status_throttle_ms: 1_000,
        max_pending: 8,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: config.router.clone(),
        custom_harnesses: presets.to_vec(),
        budget: None,
        attribution: config.attribution.commit,
    };
    let run_task: medulla::daemon::providers::RunTaskFn = Arc::new(|_| {
        Box::pin(async {
            Err("this host serves operator sessions, not dispatched tasks".to_string())
        })
    });
    let send: medulla::daemon::SendFn = Arc::new(|_, _| Box::pin(async {}));
    LocalSessions {
        // The host's own `[[hooks]]`, `[router]`, presets and attribution — so a
        // coding agent started from across the network is launched by exactly
        // the configuration that machine would use for one started on it. This
        // is the whole of what makes a remote agent a real agent rather than a
        // pty with a CLI in it.
        hooks: config.hooks.clone(),
        log: None,
        sessions: PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(daemon, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env: env.clone(),
        workspace: workspace.to_string(),
        providers,
        custom_harnesses: presets.to_vec(),
        router: config.router.clone(),
        attribution: config.attribution.commit,
    }
}

/// This machine's name, for the client's rail.
fn hostname_of(env: &HashMap<String, String>) -> Option<String> {
    env.get("HOSTNAME")
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
        })
}

/// Read `--name value` or `--name=value`.
fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == name {
            return it.next().cloned().filter(|value| !value.is_empty());
        }
        if let Some(rest) = arg.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return (!rest.is_empty()).then(|| rest.to_string());
        }
    }
    None
}
