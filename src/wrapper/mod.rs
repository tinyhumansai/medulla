//! The transparent harness wrapper behind `medulla codex` / `medulla claude` /
//! `medulla opencode`.
//!
//! The wrapper launches the real coding-agent CLI in the user's terminal exactly
//! as if it were run directly (inherited stdio — no PTY re-implementation), while
//! bridging the session to tiny.place underneath: it tails the harness's own
//! JSONL transcript, normalizes each record into a typed
//! [`SessionEnvelopeV2`](crate::tinyplace_support::SessionEnvelopeV2) event, and
//! forwards the stream as encrypted Signal DMs to the configured owner. When
//! inbound input is enabled it also polls the mailbox for owner→session control
//! frames and types their text into the child.
//!
//! This is the single-terminal `--raw` mode of the TypeScript `tinyplace codex`
//! command, ported to Rust. It reuses the existing medulla pieces rather than
//! duplicating them: transcript discovery ([`crate::session_history`]), record →
//! event mapping ([`crate::daemon::mappers`]), the derived status machine
//! ([`crate::tinyplace_support::status`]), encrypted transport
//! ([`crate::daemon::transport::SignalTransport`]), and identity/config bootstrap
//! ([`crate::tinyplace_support`]).
//!
//! ## Scope cuts (deliberately not built here)
//!
//! These parts of the TypeScript wrapper are out of scope for this single-terminal
//! slice and are intentionally omitted:
//! - the tinyplace TUI chrome mode and the `--agent` plugin mode;
//! - the **machine bus** multi-terminal coordination (wallet lock, session spool,
//!   inbox routing) — one terminal, one session, direct id matching instead;
//! - the **opencode SSE server** bridge — opencode therefore runs as a passthrough
//!   with input injection but **no transcript tailing** (its session log is not a
//!   flat JSONL the mappers read);
//! - the terminal-envelope writer (raw keystroke/output capture);
//! - `node-pty`: stdio is inherited. For a pristine full-screen TUI, run without
//!   inbound input (or `--no-bridge`) so stdin stays attached to the terminal;
//!   enabling input injection pipes stdin (a best-effort byte pump), which a
//!   full-screen TUI may not drive perfectly.

pub mod control;
pub mod envelope;
pub mod tail;

use std::collections::HashMap;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::daemon::mappers::HarnessLineMapper;
use crate::daemon::transport::SignalTransport;
use crate::session_history::SessionAgentKind;
use crate::tinyplace_support::{
    config_path, decode_task_frame, load_or_create_identity, parse_harness_control_frame,
    parse_session_envelope, reduce_status, tick_status, HarnessEvent, HarnessProvider,
    SemanticEvent, SessionStatusState,
};
use tinyplace::auth::timestamp;
use tinyplace::crypto::sha256_hex;
use tinyplace::{Signer, TinyPlaceClient, TinyPlaceClientOptions};

use control::frame_targets_session;
use envelope::EnvelopeBuilder;
use tail::SessionTailer;

const TAIL_POLL_MS: u64 = 500;
const RECEIVE_POLL_MS: u64 = 1_500;
const STATUS_THROTTLE_MS: i64 = 4_000;
const STATUS_IDLE_MS: i64 = 30_000;
const INBOX_LIMIT: i64 = 50;

/// Everything a wrapper run needs. Built from the process environment by
/// [`run_wrapper`]; constructed explicitly by tests.
pub struct WrapperConfig {
    pub provider: HarnessProvider,
    /// Arguments passed through to the child CLI verbatim.
    pub child_args: Vec<String>,
    /// The environment used for config/session-dir/bin resolution and applied as
    /// an overlay on the child's inherited environment.
    pub env: HashMap<String, String>,
    /// The working directory the child runs in and the session is anchored to.
    pub cwd: String,
    /// Pure passthrough: never activate the tiny.place bridge.
    pub no_bridge: bool,
    /// Override the generated wrapper session id (deterministic tests).
    pub session_id: Option<String>,
}

/// Parse `medulla <provider> [args…]`: strip the wrapper's own `--no-bridge`, pass
/// everything else through to the child verbatim. `--` forces the rest through.
pub fn parse_wrapper_args(args: &[String]) -> (bool, Vec<String>) {
    let mut no_bridge = false;
    let mut child: Vec<String> = Vec::new();
    let mut passthrough = false;
    for arg in args {
        if passthrough {
            child.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => passthrough = true,
            "--no-bridge" => no_bridge = true,
            _ => child.push(arg.clone()),
        }
    }
    (no_bridge, child)
}

/// The `medulla codex|claude|opencode` entry: build a [`WrapperConfig`] from the
/// process environment and run the wrapper, returning the child's exit code.
pub async fn run_wrapper(provider: HarnessProvider, args: &[String]) -> anyhow::Result<i32> {
    let (no_bridge, child_args) = parse_wrapper_args(args);
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    run_wrapper_with(WrapperConfig {
        provider,
        child_args,
        env,
        cwd,
        no_bridge,
        session_id: None,
    })
    .await
}

/// The provider's transcript agent kind, or `None` for opencode (no tailing).
fn agent_kind(provider: HarnessProvider) -> Option<SessionAgentKind> {
    match provider {
        HarnessProvider::Claude => Some(SessionAgentKind::Claude),
        HarnessProvider::Codex => Some(SessionAgentKind::Codex),
        HarnessProvider::Opencode => None,
    }
}

fn first_env<'a>(env: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a String> {
    keys.iter()
        .filter_map(|key| env.get(*key))
        .find(|value| !value.is_empty())
}

fn provider_env_key(provider: HarnessProvider, suffix: &str) -> String {
    format!("TINYPLACE_{}_{suffix}", provider.as_str().to_uppercase())
}

/// The owner this session forwards envelopes to (and, by default, receives input
/// from). Order mirrors the TS wrapper's `dmRecipient` resolution.
fn resolve_recipient(provider: HarnessProvider, env: &HashMap<String, String>) -> Option<String> {
    first_env(
        env,
        &[
            &provider_env_key(provider, "DM_TO"),
            "TINYPLACE_HARNESS_DM_TO",
            "TINYPLACE_OPENHUMAN_OWNER",
            "OPENHUMAN_OWNER_AGENT",
        ],
    )
    .cloned()
}

/// The peer whose inbound control frames / plain DMs are injected as input.
fn resolve_receive_from(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    recipient: Option<&str>,
) -> Option<String> {
    first_env(
        env,
        &[
            &provider_env_key(provider, "RECEIVE_FROM"),
            "TINYPLACE_HARNESS_RECEIVE_FROM",
        ],
    )
    .cloned()
    .or_else(|| recipient.map(str::to_string))
}

/// Inbound input is on unless `TINYPLACE_<P>_RECEIVE` / `TINYPLACE_HARNESS_RECEIVE`
/// is set to `0`.
fn receive_enabled(provider: HarnessProvider, env: &HashMap<String, String>) -> bool {
    for key in [
        provider_env_key(provider, "RECEIVE"),
        "TINYPLACE_HARNESS_RECEIVE".to_string(),
    ] {
        if let Some(value) = env.get(&key) {
            if value == "0" {
                return false;
            }
        }
    }
    true
}

/// Mint a wrapper session id: `tp-<provider>-<iso>-<rand>`, id-safe.
fn mint_session_id(provider: HarnessProvider) -> String {
    let iso = timestamp().replace([':', '.'], "-");
    let short: String = sha256_hex(tinyplace::auth::generate_nonce().as_bytes())
        .chars()
        .take(12)
        .collect();
    format!("tp-{}-{iso}-{short}", provider.as_str())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The tiny.place bridge for one wrapped session: encrypted transport plus the
/// per-session envelope/status/tailer state. Absent when running passthrough.
struct Bridge {
    transport: SignalTransport,
    recipient: Option<String>,
    receive_from: Option<String>,
    receive_active: bool,
    builder: EnvelopeBuilder,
    status: SessionStatusState,
    last_status_ms: i64,
    mapper: HarnessLineMapper,
    tailer: Option<SessionTailer>,
    wrapper_session_id: String,
    harness_session_id: String,
}

impl Bridge {
    async fn publish(&self, envelope: &crate::tinyplace_support::SessionEnvelopeV2) {
        let recipient = match &self.recipient {
            Some(recipient) => recipient,
            None => return,
        };
        let body = match serde_json::to_string(envelope) {
            Ok(body) => body,
            Err(_) => return,
        };
        if let Err(err) = self.transport.send(recipient, &body).await {
            eprintln!("medulla wrapper: publish failed: {err}");
        }
    }

    /// Emit a synthetic lifecycle envelope (`session_start` / `session_end`).
    async fn lifecycle(&mut self, phase: &str) {
        if self.recipient.is_none() {
            return;
        }
        let event = HarnessEvent {
            kind: "lifecycle".to_string(),
            role: "agent".to_string(),
            payload: serde_json::json!({ "phase": phase }),
            ..Default::default()
        };
        let envelope = self
            .builder
            .synthetic_envelope(event, "wrapper:lifecycle", now_ms());
        self.publish(&envelope).await;
    }

    /// Fold new transcript lines into events, publish them, and advance status.
    async fn ingest_lines(&mut self, lines: Vec<tail::TailLine>) {
        for line in lines {
            let semantics = self.mapper.map_line(&line.text, line.line_no);
            for semantic in semantics {
                self.advance_status(&semantic).await;
                let envelope = self.builder.event_envelope(&semantic);
                self.publish(&envelope).await;
            }
        }
    }

    async fn advance_status(&mut self, semantic: &crate::daemon::mappers::HarnessSemanticEvent) {
        let event = SemanticEvent {
            timestamp_ms: Some(semantic.timestamp_ms),
            event: semantic.event.decoded(),
        };
        let step = reduce_status(&self.status, &event);
        self.status = step.next;
        if let Some(payload) = step.emit {
            self.maybe_publish_status(payload).await;
        }
    }

    async fn tick_status(&mut self) {
        let heartbeat = now_ms().saturating_sub(self.last_status_ms) >= STATUS_THROTTLE_MS;
        let step = tick_status(&self.status, now_ms(), STATUS_IDLE_MS, heartbeat);
        self.status = step.next;
        if let Some(payload) = step.emit {
            self.maybe_publish_status(payload).await;
        }
    }

    async fn maybe_publish_status(&mut self, payload: crate::tinyplace_support::StatusPayload) {
        let now = now_ms();
        if now.saturating_sub(self.last_status_ms) < STATUS_THROTTLE_MS {
            return;
        }
        self.last_status_ms = now;
        let event = HarnessEvent {
            kind: "status".to_string(),
            role: "agent".to_string(),
            payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
            ..Default::default()
        };
        let envelope = self
            .builder
            .synthetic_envelope(event, "wrapper:status", now);
        self.publish(&envelope).await;
    }
}

/// Build the bridge, or `None` (passthrough) when it is disabled/unconfigured.
/// Prints a single warning when the bridge was wanted but cannot be configured.
async fn build_bridge(
    config: &WrapperConfig,
    wrapper_session_id: &str,
    start_ms: i64,
) -> Option<Bridge> {
    if config.no_bridge {
        return None;
    }
    let recipient = resolve_recipient(config.provider, &config.env);
    let receive_from = resolve_receive_from(config.provider, &config.env, recipient.as_deref());
    if recipient.is_none() && receive_from.is_none() {
        eprintln!(
            "medulla wrapper: no tiny.place owner configured (set TINYPLACE_HARNESS_DM_TO or TINYPLACE_OPENHUMAN_OWNER) — running as a plain passthrough"
        );
        return None;
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let config_file = config_path(&config.env, &home);
    let (signer, tp_config) = match load_or_create_identity(&config_file, &config.env) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!(
                "medulla wrapper: identity load failed ({err}) — running as a plain passthrough"
            );
            return None;
        }
    };
    let base_url = crate::tinyplace_support::resolve_endpoint(&config.env, &tp_config);
    let signer = Arc::new(signer);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url,
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let identity_dir = config_file
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".tinyplace"));
    let transport = SignalTransport::new(client, &signer, &identity_dir);
    // Publish pre-keys so the owner can open an encrypted channel to us.
    if let Err(err) = transport.publish_keys(&signer).await {
        eprintln!("medulla wrapper: pre-key publish failed: {err}");
    }

    let receive_active = receive_from.is_some() && receive_enabled(config.provider, &config.env);
    let tailer = agent_kind(config.provider)
        .map(|kind| SessionTailer::new(config.env.clone(), kind, config.cwd.clone(), start_ms));

    let mut argv = vec![crate::daemon::providers::provider_bin(
        config.provider,
        &config.env,
    )];
    argv.extend(config.child_args.iter().cloned());
    let builder = EnvelopeBuilder::new(
        wrapper_session_id,
        wrapper_session_id, // harness id defaults to the wrapper id until discovered
        config.cwd.clone(),
        config.provider.as_str(),
        argv.first().cloned().unwrap_or_default(),
        argv,
    );

    Some(Bridge {
        transport,
        recipient,
        receive_from,
        receive_active,
        builder,
        status: crate::tinyplace_support::initial_status(start_ms),
        last_status_ms: i64::MIN,
        mapper: HarnessLineMapper::new(config.provider.as_str()),
        tailer,
        wrapper_session_id: wrapper_session_id.to_string(),
        harness_session_id: wrapper_session_id.to_string(),
    })
}

/// Run the wrapper described by `config`, returning the child's exit code.
pub async fn run_wrapper_with(config: WrapperConfig) -> anyhow::Result<i32> {
    let bin = crate::daemon::providers::provider_bin(config.provider, &config.env);
    let lookup = crate::daemon::providers::make_path_lookup(&config.env);
    if !lookup(&bin) {
        anyhow::bail!(
            "coding-agent CLI '{bin}' not found on PATH (install {} or set {})",
            config.provider.as_str(),
            provider_env_key(config.provider, "BIN"),
        );
    }

    let start_ms = now_ms();
    let wrapper_session_id = config
        .session_id
        .clone()
        .unwrap_or_else(|| mint_session_id(config.provider));

    let mut bridge = build_bridge(&config, &wrapper_session_id, start_ms).await;
    let receive_active = bridge.as_ref().map(|b| b.receive_active).unwrap_or(false);

    // Spawn the child. stdout/stderr are always inherited (the user interacts with
    // the real CLI). stdin is piped only when we must inject input — otherwise it
    // is inherited so a full-screen TUI stays fully interactive.
    let mut command = Command::new(&bin);
    command
        .args(&config.child_args)
        .envs(&config.env)
        .current_dir(&config.cwd)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    if receive_active {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::inherit());
    }
    let mut child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("failed to start {bin}: {err}"))?;

    // Child stdin writer: a single task owns the pipe; injection and the raw
    // stdin pump feed it over a channel.
    let stdin_tx = if receive_active {
        child.stdin.take().map(|mut child_stdin| {
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            tokio::spawn(async move {
                while let Some(bytes) = rx.recv().await {
                    if child_stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = child_stdin.flush().await;
                }
            });
            tx
        })
    } else {
        None
    };
    // Forward the real terminal's stdin to the child (best-effort byte pump), only
    // when a TTY is attached so tests / pipes never consume the parent's stdin.
    if let Some(tx) = &stdin_tx {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut stdin = std::io::stdin();
                let mut buf = [0u8; 1024];
                loop {
                    match stdin.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    }

    if let Some(bridge) = bridge.as_mut() {
        bridge.lifecycle("session_start").await;
    }

    let mut tail_tick = tokio::time::interval(Duration::from_millis(TAIL_POLL_MS));
    let mut recv_tick = tokio::time::interval(Duration::from_millis(RECEIVE_POLL_MS));
    let mut status_tick = tokio::time::interval(Duration::from_millis(STATUS_THROTTLE_MS as u64));
    let mut signal_fut = signal_future();

    let status = loop {
        tokio::select! {
            result = child.wait() => {
                break result;
            }
            _ = tail_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    pump_tailer(bridge).await;
                }
            }
            _ = recv_tick.tick() => {
                if let (Some(bridge), Some(tx)) = (bridge.as_mut(), stdin_tx.as_ref()) {
                    drain_and_inject(bridge, tx).await;
                }
            }
            _ = status_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    bridge.tick_status().await;
                }
            }
            _ = &mut signal_fut => {
                let _ = child.start_kill();
            }
        }
    };

    // Teardown: final transcript drain, then the closing lifecycle event.
    if let Some(bridge) = bridge.as_mut() {
        if let Some(mut tailer) = bridge.tailer.take() {
            let lines = tailer.drain();
            sync_harness_id(bridge);
            bridge.ingest_lines(lines).await;
        }
        bridge.lifecycle("session_end").await;
    }

    let code = exit_code(status?);
    Ok(code)
}

/// Poll the tailer, latch the harness id on first sighting, and ingest new lines.
async fn pump_tailer(bridge: &mut Bridge) {
    let mut tailer = match bridge.tailer.take() {
        Some(tailer) => tailer,
        None => return,
    };
    let poll = tailer.poll();
    if let Some(located) = &poll.located {
        bridge.harness_session_id = located.harness_session_id.clone();
        bridge
            .builder
            .set_harness_session_id(located.harness_session_id.clone());
        bridge
            .builder
            .set_source_path(located.path.to_string_lossy().into_owned());
    }
    let lines = poll.lines;
    bridge.tailer = Some(tailer);
    bridge.ingest_lines(lines).await;
}

fn sync_harness_id(bridge: &mut Bridge) {
    let id = bridge.harness_session_id.clone();
    bridge.builder.set_harness_session_id(id);
}

/// Drain the inbox and inject any input frames / plain owner DMs into the child.
async fn drain_and_inject(bridge: &mut Bridge, stdin_tx: &mpsc::UnboundedSender<Vec<u8>>) {
    let inbound = bridge.transport.drain_inbox(INBOX_LIMIT).await;
    for message in inbound {
        let text = classify_inbound(bridge, &message);
        if let Some(text) = text {
            let mut bytes = text.into_bytes();
            bytes.push(b'\n');
            let _ = stdin_tx.send(bytes);
        }
    }
}

/// Decide what (if anything) an inbound DM injects: a matching control frame's
/// text, or a plain owner DM verbatim. Session envelopes and task frames are
/// never injected.
fn classify_inbound(
    bridge: &Bridge,
    message: &crate::daemon::transport::InboundMessage,
) -> Option<String> {
    if let Some(frame) = parse_harness_control_frame(&message.text) {
        if frame_targets_session(
            &frame,
            &bridge.wrapper_session_id,
            &bridge.harness_session_id,
        ) {
            return Some(frame.text);
        }
        return None;
    }
    // Plain text from the configured owner only, and never a structured frame.
    let from_owner = bridge
        .receive_from
        .as_deref()
        .map(|owner| owner == message.from)
        .unwrap_or(false);
    if !from_owner || message.text.trim().is_empty() {
        return None;
    }
    if parse_session_envelope(&message.text).is_some() || decode_task_frame(&message.text).is_some()
    {
        return None;
    }
    Some(message.text.clone())
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// A future that resolves on SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
fn signal_future() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(mut sigint), Ok(mut sigterm)) => Box::pin(async move {
                tokio::select! {
                    _ = sigint.recv() => {}
                    _ = sigterm.recv() => {}
                }
            }),
            _ => Box::pin(std::future::pending()),
        }
    }
    #[cfg(not(unix))]
    {
        Box::pin(async move {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_wrapper_args_strips_no_bridge_and_passes_rest() {
        let (no_bridge, child) =
            parse_wrapper_args(&argv(&["--no-bridge", "resume", "--model", "x"]));
        assert!(no_bridge);
        assert_eq!(child, vec!["resume", "--model", "x"]);

        let (no_bridge, child) = parse_wrapper_args(&argv(&["exec", "--json"]));
        assert!(!no_bridge);
        assert_eq!(child, vec!["exec", "--json"]);
    }

    #[test]
    fn double_dash_forces_passthrough_including_no_bridge() {
        let (no_bridge, child) = parse_wrapper_args(&argv(&["--", "--no-bridge", "--flag"]));
        assert!(!no_bridge, "after -- everything is the child's");
        assert_eq!(child, vec!["--no-bridge", "--flag"]);
    }

    #[test]
    fn agent_kind_maps_providers() {
        assert_eq!(
            agent_kind(HarnessProvider::Claude),
            Some(SessionAgentKind::Claude)
        );
        assert_eq!(
            agent_kind(HarnessProvider::Codex),
            Some(SessionAgentKind::Codex)
        );
        assert_eq!(agent_kind(HarnessProvider::Opencode), None);
    }

    #[test]
    fn recipient_and_receive_resolution_order() {
        let mut env = HashMap::new();
        env.insert(
            "TINYPLACE_OPENHUMAN_OWNER".to_string(),
            "owner-a".to_string(),
        );
        assert_eq!(
            resolve_recipient(HarnessProvider::Codex, &env).as_deref(),
            Some("owner-a")
        );
        // A per-provider override wins.
        env.insert(
            "TINYPLACE_CODEX_DM_TO".to_string(),
            "owner-codex".to_string(),
        );
        assert_eq!(
            resolve_recipient(HarnessProvider::Codex, &env).as_deref(),
            Some("owner-codex")
        );
        // receive_from falls back to the recipient.
        assert_eq!(
            resolve_receive_from(HarnessProvider::Codex, &env, Some("owner-codex")).as_deref(),
            Some("owner-codex")
        );
        env.insert(
            "TINYPLACE_HARNESS_RECEIVE_FROM".to_string(),
            "sender-b".to_string(),
        );
        assert_eq!(
            resolve_receive_from(HarnessProvider::Codex, &env, Some("owner-codex")).as_deref(),
            Some("sender-b")
        );
    }

    #[test]
    fn receive_disabled_by_zero() {
        let mut env = HashMap::new();
        assert!(receive_enabled(HarnessProvider::Claude, &env));
        env.insert("TINYPLACE_CLAUDE_RECEIVE".to_string(), "0".to_string());
        assert!(!receive_enabled(HarnessProvider::Claude, &env));
    }

    #[test]
    fn mint_session_id_is_id_safe_and_prefixed() {
        let id = mint_session_id(HarnessProvider::Codex);
        assert!(id.starts_with("tp-codex-"));
        assert!(!id.contains(':'));
        assert!(!id.contains('.'));
    }

    #[tokio::test]
    async fn missing_binary_is_a_clear_error() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/nonexistent".to_string());
        env.insert(
            "TINYPLACE_CODEX_BIN".to_string(),
            "/no/such/codex-binary".to_string(),
        );
        let err = run_wrapper_with(WrapperConfig {
            provider: HarnessProvider::Codex,
            child_args: Vec::new(),
            env,
            cwd: ".".to_string(),
            no_bridge: true,
            session_id: Some("wsid-test".to_string()),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found on PATH"), "got: {err}");
    }
}
