//! The tiny.place bridge for one wrapped session and its I/O helpers.
//!
//! [`Bridge`] holds the encrypted transport plus the per-session
//! envelope/status/tailer state; [`build_bridge`] constructs it (or returns
//! `None` for a plain passthrough). The free functions here fold transcript lines
//! into events ([`pump_tailer`]) and route inbound owner DMs into the child
//! ([`drain_and_inject`] / [`classify_inbound`]). The process orchestration that
//! drives these lives in [`run`](super::run).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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

use super::control::frame_targets_session;
use super::envelope::EnvelopeBuilder;
use super::tail::{SessionTailer, TailLine};
use super::types::{WrapperConfig, WrapperTimings};

/// Maximum inbound messages drained from the mailbox per receive tick.
const INBOX_LIMIT: i64 = 50;

/// The provider's transcript agent kind, or `None` for opencode (no tailing).
pub(super) fn agent_kind(provider: HarnessProvider) -> Option<SessionAgentKind> {
    match provider {
        HarnessProvider::Claude => Some(SessionAgentKind::Claude),
        HarnessProvider::Codex => Some(SessionAgentKind::Codex),
        HarnessProvider::Opencode => None,
    }
}

/// The `TINYPLACE_<P>_BIN` env key, for the missing-binary error hint.
pub(super) fn provider_bin_env_key(provider: HarnessProvider) -> String {
    format!("TINYPLACE_{}_BIN", provider.as_str().to_uppercase())
}

/// The owner this session forwards envelopes to: the central env chain, with the
/// persisted worker profile's owner as the final fallback (env always wins).
pub(super) fn resolve_recipient(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    profile_owner: Option<&str>,
) -> Option<String> {
    crate::tinyplace_support::env::dm_recipient(provider, env)
        .or_else(|| profile_owner.map(str::to_string).filter(|s| !s.is_empty()))
}

/// Mint a wrapper session id: `tp-<provider>-<iso>-<rand>`, id-safe.
pub(super) fn mint_session_id(provider: HarnessProvider) -> String {
    let iso = timestamp().replace([':', '.'], "-");
    let short: String = sha256_hex(tinyplace::auth::generate_nonce().as_bytes())
        .chars()
        .take(12)
        .collect();
    format!("tp-{}-{iso}-{short}", provider.as_str())
}

/// Milliseconds since the Unix epoch (0 on a clock error).
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The tiny.place bridge for one wrapped session: encrypted transport plus the
/// per-session envelope/status/tailer state. Absent when running passthrough.
pub(super) struct Bridge {
    pub(super) transport: SignalTransport,
    pub(super) recipient: Option<String>,
    pub(super) receive_from: Option<String>,
    pub(super) receive_active: bool,
    pub(super) builder: EnvelopeBuilder,
    pub(super) status: SessionStatusState,
    pub(super) last_status_ms: i64,
    pub(super) mapper: HarnessLineMapper,
    pub(super) tailer: Option<SessionTailer>,
    pub(super) wrapper_session_id: String,
    pub(super) harness_session_id: String,
    pub(super) status_throttle_ms: i64,
    pub(super) status_idle_ms: i64,
}

impl Bridge {
    /// Serialize and send `envelope` to the configured recipient (no-op when the
    /// bridge has no recipient or serialization fails).
    pub(super) async fn publish(&self, envelope: &crate::tinyplace_support::SessionEnvelopeV2) {
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
    pub(super) async fn lifecycle(&mut self, phase: &str) {
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
    pub(super) async fn ingest_lines(&mut self, lines: Vec<TailLine>) {
        for line in lines {
            let semantics = self.mapper.map_line(&line.text, line.line_no);
            for semantic in semantics {
                self.advance_status(&semantic).await;
                let envelope = self.builder.event_envelope(&semantic);
                self.publish(&envelope).await;
            }
        }
    }

    /// Fold one semantic event into the status machine, publishing a status
    /// envelope when it emits one.
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

    /// Advance the status machine on a timer tick (heartbeat / idle transition).
    pub(super) async fn tick_status(&mut self) {
        let heartbeat = now_ms().saturating_sub(self.last_status_ms) >= self.status_throttle_ms;
        let step = tick_status(&self.status, now_ms(), self.status_idle_ms, heartbeat);
        self.status = step.next;
        if let Some(payload) = step.emit {
            self.maybe_publish_status(payload).await;
        }
    }

    /// Publish a status envelope unless the throttle window is still open.
    async fn maybe_publish_status(&mut self, payload: crate::tinyplace_support::StatusPayload) {
        let now = now_ms();
        if now.saturating_sub(self.last_status_ms) < self.status_throttle_ms {
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
pub(super) async fn build_bridge(
    config: &WrapperConfig,
    wrapper_session_id: &str,
    start_ms: i64,
) -> Option<Bridge> {
    if config.no_bridge {
        return None;
    }
    use crate::tinyplace_support::env as tp_env;
    // The persisted worker profile's owner is the recipient fallback when no env
    // owner is set (env still wins).
    let profile = crate::worker_profile::WorkerProfile::load(&crate::worker_profile::profile_path(
        &config.env,
    ));
    let profile_owner = profile.as_ref().and_then(|p| p.owner.as_deref());
    let recipient = resolve_recipient(config.provider, &config.env, profile_owner);
    let receive_from = tp_env::receive_from(config.provider, &config.env, recipient.as_deref());
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

    let receive_active =
        receive_from.is_some() && tp_env::receive_enabled(config.provider, &config.env);
    let tailer = agent_kind(config.provider)
        .map(|kind| SessionTailer::new(config.env.clone(), kind, config.cwd.clone(), start_ms));

    let timings = WrapperTimings::resolve(config.provider, &config.env);
    let mut argv = vec![tp_env::provider_bin(config.provider, &config.env)];
    argv.extend(tp_env::provider_args(config.provider, &config.env));
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
        status_throttle_ms: timings.status_throttle_ms,
        status_idle_ms: timings.status_idle_ms,
    })
}

/// Poll the tailer, latch the harness id on first sighting, and ingest new lines.
pub(super) async fn pump_tailer(bridge: &mut Bridge) {
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

/// Copy the latched harness session id into the envelope builder before a final
/// transcript drain.
pub(super) fn sync_harness_id(bridge: &mut Bridge) {
    let id = bridge.harness_session_id.clone();
    bridge.builder.set_harness_session_id(id);
}

/// Drain the inbox and inject any input frames / plain owner DMs into the child.
pub(super) async fn drain_and_inject(
    bridge: &mut Bridge,
    stdin_tx: &mpsc::UnboundedSender<Vec<u8>>,
) {
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
