//! The host-link bridge for one wrapped session and its I/O helpers.
//!
//! [`Bridge`] holds the transport plus the per-session
//! envelope/status/tailer state; [`build_bridge`] constructs it (or returns
//! `None` for a plain passthrough). The free functions here fold transcript lines
//! into events ([`pump_tailer`]) and route inbound owner DMs into the child
//! ([`drain_and_inject`] / [`classify_inbound`]). The process orchestration that
//! drives these lives in [`run`](super::run).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use medulla_link::{Link, LinkConfig};

use crate::bridge::LinkBridge;
use crate::clock::iso_now;
use crate::daemon::mappers::HarnessLineMapper;
use crate::protocol::{
    decode_task_frame, parse_harness_control_frame, parse_screen_message, parse_session_envelope,
    reduce_status, tick_status, HarnessEvent, HarnessProvider, SemanticEvent, SessionStatusState,
};
use crate::session_history::SessionAgentKind;
use crate::update::sha256_hex;

use super::control::frame_targets_session;
use super::envelope::EnvelopeBuilder;
use super::tail::{SessionTailer, TailLine};
use super::types::{WrapperConfig, WrapperTimings};

/// Maximum inbound messages drained from the mailbox per receive tick.
const INBOX_LIMIT: i64 = 50;

/// Publications buffered away from child and signal supervision.
const PUBLISH_CAPACITY: usize = 256;

/// The provider's transcript agent kind, or `None` for opencode (no tailing).
pub(super) fn agent_kind(provider: HarnessProvider) -> Option<SessionAgentKind> {
    match provider {
        HarnessProvider::Claude => Some(SessionAgentKind::Claude),
        HarnessProvider::Codex => Some(SessionAgentKind::Codex),
        HarnessProvider::Opencode | HarnessProvider::Openhuman => None,
    }
}

/// The `MEDULLA_<P>_BIN` env key, for the missing-binary error hint.
pub(super) fn provider_bin_env_key(provider: HarnessProvider) -> String {
    format!("MEDULLA_{}_BIN", provider.as_str().to_uppercase())
}

/// The owner this session forwards envelopes to: the central env chain, with the
/// persisted worker profile's owner as the final fallback (env always wins).
pub(super) fn resolve_recipient(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    profile_owner: Option<&str>,
) -> Option<String> {
    crate::protocol::env::dm_recipient(provider, env)
        .or_else(|| profile_owner.map(str::to_string).filter(|s| !s.is_empty()))
}

/// Mint a wrapper session id: `tp-<provider>-<iso>-<rand>`, id-safe.
///
/// The random suffix disambiguates two sessions started in the same
/// millisecond; it is a uniqueness tiebreak, not a secret.
pub(super) fn mint_session_id(provider: HarnessProvider) -> String {
    let iso = iso_now().replace([':', '.'], "-");
    let short: String = sha256_hex(uuid::Uuid::new_v4().as_bytes())
        .chars()
        .take(12)
        .collect();
    format!("tp-{}-{iso}-{short}", provider.as_str())
}

/// Milliseconds since the Unix epoch (0 on a clock error). Delegates to the
/// shared [`crate::clock`] helper.
pub(super) fn now_ms() -> i64 {
    crate::clock::now_millis()
}

impl Bridge {
    /// Serialize and send `envelope` to the configured recipient (no-op when the
    /// bridge has no recipient or serialization fails).
    ///
    /// Enqueues without waiting for link backpressure. A dedicated publisher
    /// task owns reliable link retries, so an offline recipient cannot stop the
    /// wrapper from polling child completion or signals. A full publication
    /// queue drops the new envelope; the tailer has already consumed the event,
    /// so missing envelopes after a long outage are accepted rather than
    /// blocking supervision.
    pub(super) async fn publish(&self, envelope: &crate::protocol::SessionEnvelopeV2) {
        if self.recipient.is_none() {
            return;
        }
        let body = match serde_json::to_string(envelope) {
            Ok(body) => body,
            Err(_) => return,
        };
        let Some(publish_tx) = self.publish_tx.as_ref() else {
            return;
        };
        if let Err(err) = publish_tx.try_send(body) {
            eprintln!("medulla wrapper: publication dropped: {err}");
        }
    }

    /// Close the queue and wait briefly for its reliable sender to catch up.
    pub(super) async fn finish_publications(&mut self, timeout: tokio::time::Duration) {
        self.publish_tx.take();
        if let Some(mut publisher) = self.publisher.take() {
            if tokio::time::timeout(timeout, &mut publisher).await.is_err() {
                publisher.abort();
            }
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
        if let Some(payload) = advance_status_state(&mut self.status, &event) {
            self.publish_status(payload).await;
        }
    }

    /// Advance the status machine on a timer tick (heartbeat / idle transition).
    pub(super) async fn tick_status(&mut self) {
        let now = now_ms();
        let heartbeat = heartbeat_due(self.last_status_ms, now, self.status_throttle_ms);
        let step = tick_status(&self.status, now, self.status_idle_ms, heartbeat);
        self.status = step.next;
        if let Some(payload) = step.emit {
            self.publish_status(payload).await;
        }
    }

    /// Publish a status envelope emitted by the change-gated status machine.
    ///
    /// Heartbeat rate limiting happens before this method in [`tick_status`].
    /// Applying it here would also suppress real state changes, including the
    /// first `working` event after the timer's initial `idle` heartbeat.
    async fn publish_status(&mut self, payload: crate::protocol::StatusPayload) {
        let now = now_ms();
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

/// Apply one semantic event without publication policy; state changes are
/// returned immediately so heartbeat timing cannot suppress them.
fn advance_status_state(
    status: &mut SessionStatusState,
    event: &SemanticEvent,
) -> Option<crate::protocol::StatusPayload> {
    let step = reduce_status(status, event);
    *status = step.next;
    step.emit
}

/// Whether an unchanged-status heartbeat is due at `now_ms`.
fn heartbeat_due(last_status_ms: i64, now_ms: i64, interval_ms: i64) -> bool {
    now_ms.saturating_sub(last_status_ms) >= interval_ms
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
    use crate::protocol::env as tp_env;
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
            "medulla wrapper: no link owner configured (set MEDULLA_HARNESS_DM_TO or MEDULLA_OPENHUMAN_OWNER) — running as a plain passthrough"
        );
        return None;
    }

    // The host end of the link. Without an enrolled identity there is nothing to
    // forward to, so the session runs as a plain passthrough rather than failing
    // — a harness must still be usable on a machine that was never enrolled.
    let home = crate::home::medulla_home(&config.env);
    // Load the effective configuration to honor the configured link.stateDir if set.
    let explicit_config = crate::config::explicit_config_from_env(&config.env);
    if explicit_config.is_some_and(|path| !std::path::Path::new(path).is_file()) {
        eprintln!(
            "medulla wrapper: explicit configuration does not exist — running as a plain passthrough"
        );
        return None;
    }
    let link_state_dir = match crate::config::load_config(
        explicit_config,
        &config.env,
        std::path::Path::new(&config.cwd),
    ) {
        Ok(loaded) => loaded
            .config
            .link
            .map(|link_cfg| link_cfg.state_dir.into())
            .unwrap_or_else(|| medulla_link::keys::link_dir(&home)),
        Err(err) if explicit_config.is_some() => {
            eprintln!(
                "medulla wrapper: explicit configuration failed to load ({err}) — running as a plain passthrough"
            );
            return None;
        }
        Err(_) => medulla_link::keys::link_dir(&home),
    };
    let link = match Link::connect(LinkConfig::new(link_state_dir)).await {
        Ok(link) => link,
        Err(err) => {
            eprintln!(
                "medulla wrapper: host link unavailable ({err}) — running as a plain passthrough"
            );
            return None;
        }
    };
    // The owner is the link's single peer: a host enrolls against exactly one
    // orchestrator (protocol §7.3), and the recipient env var names it.
    let owner = recipient.clone().or_else(|| receive_from.clone())?;
    let transport = match LinkBridge::single_peer(Arc::new(link), owner.clone(), owner) {
        Ok(bridge) => bridge,
        Err(err) => {
            eprintln!(
                "medulla wrapper: host link is misconfigured ({err}) — running as a plain passthrough"
            );
            return None;
        }
    };
    let (publish_tx, mut publish_rx) = mpsc::channel::<String>(PUBLISH_CAPACITY);
    let publish_transport = transport.clone();
    let publish_recipient = recipient.clone();
    let publisher = tokio::spawn(async move {
        use crate::bridge::Bridge as _;
        while let Some(body) = publish_rx.recv().await {
            let Some(recipient) = publish_recipient.as_deref() else {
                continue;
            };
            if let Err(err) = publish_transport.send(recipient, &body).await {
                eprintln!("medulla wrapper: publish failed: {err}");
            }
        }
    });

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
        publish_tx: Some(publish_tx),
        publisher: Some(publisher),
        recipient,
        receive_from,
        receive_active,
        builder,
        status: crate::protocol::initial_status(start_ms),
        last_status_ms: i64::MIN,
        mapper: wrapper_line_mapper(
            config.provider.as_str(),
            &config.env,
            std::env::var_os("GH_REPO").is_some(),
        ),
        tailer,
        wrapper_session_id: wrapper_session_id.to_string(),
        harness_session_id: wrapper_session_id.to_string(),
        status_throttle_ms: timings.status_throttle_ms,
        status_idle_ms: timings.status_idle_ms,
    })
}

/// Build the transcript mapper using the wrapped child's effective environment.
///
/// Wrapper configuration overlays, rather than clears, the host environment,
/// so the effective child value may come from either source.
fn wrapper_line_mapper(
    provider: &str,
    env: &HashMap<String, String>,
    host_gh_repo_is_set: bool,
) -> HarnessLineMapper {
    HarnessLineMapper::new_with_gh_repo_override(
        provider,
        env.contains_key("GH_REPO") || host_gh_repo_is_set,
    )
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
    use crate::bridge::Bridge as _;
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
fn classify_inbound(bridge: &Bridge, message: &crate::bridge::InboundMessage) -> Option<String> {
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
    if is_structured_frame(&message.text) {
        return None;
    }
    Some(message.text.clone())
}

/// Whether `body` belongs to one of the structured protocols that share this
/// channel, and so must never be injected into the child as prompt text.
///
/// The channel carries four: task frames, harness control frames, session
/// envelopes, and screen messages. Control frames are handled before this point
/// (they carry text that *is* meant to be injected); the rest are recognised
/// here. Keeping the set in one predicate is the point — the omission this
/// replaced silently typed `medulla.screen.v1` subscribes into a live harness,
/// because a body no parser claims is indistinguishable from an owner's DM.
fn is_structured_frame(body: &str) -> bool {
    parse_session_envelope(body).is_some()
        || decode_task_frame(body).is_some()
        || parse_screen_message(body).is_some()
}

#[cfg(test)]
mod tests;
mod types;
pub(super) use types::Bridge;
