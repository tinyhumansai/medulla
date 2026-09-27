//! The OpenPanel wire payloads, in the shape the `openpanel_rust` SDK sends.
//!
//! The SDK posts `{"type": "track" | "identify", "payload": {...}}` with
//! camelCase fields and string-valued properties; these types serialize to
//! exactly that. Two deliberate differences, both about privacy:
//!
//! - An identify carries only `profileId`. The SDK's `IdentifyUser` always
//!   sends `email`, `firstName`, and `lastName`, which Medulla must not send
//!   (and sending them blank would clobber a profile's real values).
//! - A track with no profile omits `profileId` instead of sending `null`.

use std::collections::BTreeMap;

use serde::Serialize;

/// One request body for OpenPanel's `/track` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub(super) enum Payload {
    /// A named event.
    Track(TrackPayload),
    /// Ties later events to an account id.
    Identify(IdentifyPayload),
}

/// The body of a `track` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrackPayload {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) profile_id: Option<String>,
    pub(super) properties: BTreeMap<String, String>,
}

/// The body of an `identify` payload: the opaque account id and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IdentifyPayload {
    pub(super) profile_id: String,
}

impl Payload {
    /// An identify for `user_id` with no profile attributes.
    pub(super) fn identify(user_id: &str) -> Self {
        Self::Identify(IdentifyPayload {
            profile_id: user_id.to_owned(),
        })
    }

    /// A `name` event with an explicitly allowlisted property set, plus the
    /// app name and version every event carries.
    pub(super) fn track<'a>(
        name: &str,
        profile_id: Option<&str>,
        properties: impl IntoIterator<Item = (&'a str, String)>,
    ) -> Self {
        let mut properties: BTreeMap<String, String> = properties
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        properties.insert("app".into(), "medulla".into());
        properties.insert("version".into(), env!("CARGO_PKG_VERSION").into());
        Self::Track(TrackPayload {
            name: name.to_owned(),
            profile_id: profile_id.map(str::to_owned),
            properties,
        })
    }
}
