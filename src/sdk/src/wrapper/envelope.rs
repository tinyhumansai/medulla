//! v2 session-envelope construction for the harness wrapper.
//!
//! A wrapped session forwards its activity as a stream of
//! [`SessionEnvelopeV2`] packets (the SDK wire type). This module is the pure,
//! deterministic builder behind that stream: it stamps a monotonic `seq`, floors
//! the bucket to the minute, and derives the idempotent `event.id`
//! (`sha256(wrapperSessionId\0recordType\0seq\0kind)`) exactly as the TypeScript
//! wrapper does. No I/O and no clock of its own beyond the timestamps it is
//! handed, so every field is unit-testable.

use tinyplace::crypto::sha256_hex;
use tinyplace::types::{
    HarnessBucket, HarnessEvent, HarnessInfo, HarnessScope, HarnessSource, SessionEnvelopeV2,
    SESSION_ENVELOPE_VERSION_V2,
};

use crate::daemon::mappers::HarnessSemanticEvent;

/// The stable per-run facts an envelope carries, plus the advancing `seq`.
pub struct EnvelopeBuilder {
    wrapper_session_id: String,
    harness_session_id: String,
    cwd: String,
    provider: String,
    command: String,
    argv: Vec<String>,
    source_path: String,
    seq: i64,
}

impl EnvelopeBuilder {
    /// Start a builder. `harness_session_id` may be the wrapper id until the real
    /// transcript id is discovered (kept non-empty so envelopes stay valid).
    pub fn new(
        wrapper_session_id: impl Into<String>,
        harness_session_id: impl Into<String>,
        cwd: impl Into<String>,
        provider: impl Into<String>,
        command: impl Into<String>,
        argv: Vec<String>,
    ) -> Self {
        EnvelopeBuilder {
            wrapper_session_id: wrapper_session_id.into(),
            harness_session_id: harness_session_id.into(),
            cwd: cwd.into(),
            provider: provider.into(),
            command: command.into(),
            argv,
            source_path: String::new(),
            seq: 0,
        }
    }

    /// The next seq that will be assigned (0-based, monotonic).
    pub fn next_seq(&self) -> i64 {
        self.seq
    }

    /// Update the real harness session id once the transcript is located.
    pub fn set_harness_session_id(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !id.is_empty() {
            self.harness_session_id = id;
        }
    }

    /// Point subsequent envelopes at the discovered transcript path.
    pub fn set_source_path(&mut self, path: impl Into<String>) {
        self.source_path = path.into();
    }

    /// Build a v2 envelope from a mapped transcript event, advancing `seq`.
    pub fn event_envelope(&mut self, semantic: &HarnessSemanticEvent) -> SessionEnvelopeV2 {
        let ts = epoch_ms_to_iso(semantic.timestamp_ms);
        let mut event = semantic.event.clone();
        self.stamp_event(&mut event, &semantic.record_type, &ts);
        self.build(event, &semantic.record_type, &ts)
    }

    /// Build a synthetic envelope for a wrapper-generated event (lifecycle or the
    /// derived status), advancing `seq`. `record_type` labels the source.
    pub fn synthetic_envelope(
        &mut self,
        mut event: HarnessEvent,
        record_type: &str,
        timestamp_ms: i64,
    ) -> SessionEnvelopeV2 {
        let ts = epoch_ms_to_iso(timestamp_ms);
        self.stamp_event(&mut event, record_type, &ts);
        self.build(event, record_type, &ts)
    }

    fn stamp_event(&mut self, event: &mut HarnessEvent, record_type: &str, ts: &str) {
        let seq = self.seq;
        self.seq += 1;
        event.seq = seq;
        event.ts = ts.to_string();
        event.id = event_id(&self.wrapper_session_id, record_type, seq, &event.kind);
    }

    fn build(&self, event: HarnessEvent, record_type: &str, ts: &str) -> SessionEnvelopeV2 {
        SessionEnvelopeV2 {
            envelope_version: SESSION_ENVELOPE_VERSION_V2.to_string(),
            version: 2,
            bucket: minute_bucket(ts),
            scope: HarnessScope {
                scope_type: "session".to_string(),
                key: self.wrapper_session_id.clone(),
                cwd: self.cwd.clone(),
                wrapper_session_id: self.wrapper_session_id.clone(),
                harness_session_id: self.harness_session_id.clone(),
            },
            harness: HarnessInfo {
                provider: self.provider.clone(),
                command: self.command.clone(),
                argv: self.argv.clone(),
            },
            event,
            source: HarnessSource {
                path: self.source_path.clone(),
                record_type: record_type.to_string(),
                source_role: None,
            },
        }
    }
}

/// The idempotent event id: `sha256(wsid\0recordType\0seq\0kind)`, hex.
pub fn event_id(wrapper_session_id: &str, record_type: &str, seq: i64, kind: &str) -> String {
    let material = format!("{wrapper_session_id}\0{record_type}\0{seq}\0{kind}");
    sha256_hex(material.as_bytes())
}

/// A minute-floored bucket for the ISO instant `ts`. Start is `ts` truncated to
/// the minute; end is start + 60s. Falls back to a zero bucket for an unparseable
/// timestamp so a bad clock never sinks the envelope.
pub fn minute_bucket(ts: &str) -> HarnessBucket {
    match iso_to_epoch_ms(ts) {
        Some(ms) => {
            let start_ms = ms - ms.rem_euclid(60_000);
            HarnessBucket {
                unit: "minute".to_string(),
                start: epoch_ms_to_iso(start_ms),
                end: epoch_ms_to_iso(start_ms + 60_000),
            }
        }
        None => HarnessBucket {
            unit: "minute".to_string(),
            start: ts.to_string(),
            end: ts.to_string(),
        },
    }
}

// ── dependency-free epoch <-> ISO (UTC, millisecond) ─────────────────────────

/// Format epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.sssZ`.
pub fn epoch_ms_to_iso(ms: i64) -> String {
    let total_secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Parse an RFC3339 UTC-ish instant to epoch ms. A thin reuse-shaped wrapper: the
/// full parser lives in the mappers module, so we go through JSON to reach it.
fn iso_to_epoch_ms(ts: &str) -> Option<i64> {
    crate::daemon::mappers::parse_iso_ms(ts)
}

/// Civil date (year, month, day) from days since the Unix epoch — the inverse of
/// `days_from_civil` (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::mappers::HarnessLineMapper;

    fn builder() -> EnvelopeBuilder {
        EnvelopeBuilder::new(
            "wsid-1",
            "wsid-1",
            "/repo",
            "claude",
            "claude",
            vec!["-p".to_string()],
        )
    }

    #[test]
    fn epoch_iso_round_trips() {
        // 2026-07-05T00:00:00.000Z is 1_783_209_600_000 ms.
        assert_eq!(
            epoch_ms_to_iso(1_783_209_600_000),
            "2026-07-05T00:00:00.000Z"
        );
        assert_eq!(
            epoch_ms_to_iso(1_783_209_600_500),
            "2026-07-05T00:00:00.500Z"
        );
        assert_eq!(epoch_ms_to_iso(0), "1970-01-01T00:00:00.000Z");
        // Round-trip through the mapper's parser.
        let ms = 1_783_209_671_123;
        assert_eq!(iso_to_epoch_ms(&epoch_ms_to_iso(ms)), Some(ms));
    }

    #[test]
    fn bucket_floors_to_the_minute() {
        let bucket = minute_bucket("2026-07-05T12:34:56.789Z");
        assert_eq!(bucket.unit, "minute");
        assert_eq!(bucket.start, "2026-07-05T12:34:00.000Z");
        assert_eq!(bucket.end, "2026-07-05T12:35:00.000Z");
    }

    #[test]
    fn event_id_is_stable_and_position_sensitive() {
        let a = event_id("wsid-1", "assistant:text", 3, "agent_message");
        let b = event_id("wsid-1", "assistant:text", 3, "agent_message");
        assert_eq!(a, b, "same inputs → same id");
        assert_ne!(a, event_id("wsid-1", "assistant:text", 4, "agent_message"));
        assert_ne!(a, event_id("wsid-2", "assistant:text", 3, "agent_message"));
        assert_eq!(a.len(), 64, "sha256 hex");
    }

    #[test]
    fn seq_advances_and_stamps_ids() {
        let mut b = builder();
        assert_eq!(b.next_seq(), 0);
        let mut mapper = HarnessLineMapper::new("claude");
        let line = r#"{"type":"assistant","timestamp":"2026-07-05T00:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        let events = mapper.map_line(line, 0);
        let env0 = b.event_envelope(&events[0]);
        assert_eq!(env0.event.seq, 0);
        assert_eq!(env0.event.kind, "agent_message");
        assert_eq!(env0.scope.wrapper_session_id, "wsid-1");
        assert_eq!(env0.scope.scope_type, "session");
        assert_eq!(env0.harness.provider, "claude");
        assert_eq!(env0.source.record_type, "assistant:text");
        assert_eq!(
            env0.event.id,
            event_id("wsid-1", "assistant:text", 0, "agent_message")
        );
        // Next envelope advances seq.
        let env1 = b.event_envelope(&events[0]);
        assert_eq!(env1.event.seq, 1);
        assert_eq!(b.next_seq(), 2);
        // Wire round-trips as a valid v2 envelope.
        let wire = serde_json::to_string(&env0).unwrap();
        assert!(SessionEnvelopeV2::parse(&wire).is_some());
    }

    #[test]
    fn harness_id_update_keeps_envelopes_valid() {
        let mut b = builder();
        b.set_harness_session_id("");
        assert_eq!(
            b.synthetic_envelope(
                HarnessEvent {
                    kind: "lifecycle".to_string(),
                    role: "agent".to_string(),
                    payload: serde_json::json!({ "phase": "session_start" }),
                    ..Default::default()
                },
                "wrapper:lifecycle",
                0,
            )
            .scope
            .harness_session_id,
            "wsid-1",
            "empty id ignored, fallback retained",
        );
        b.set_harness_session_id("real-id");
        let env = b.synthetic_envelope(
            HarnessEvent {
                kind: "lifecycle".to_string(),
                role: "agent".to_string(),
                payload: serde_json::json!({ "phase": "session_end" }),
                ..Default::default()
            },
            "wrapper:lifecycle",
            0,
        );
        assert_eq!(env.scope.harness_session_id, "real-id");
    }
}
