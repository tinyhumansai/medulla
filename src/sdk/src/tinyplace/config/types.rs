//! Data types for the `config` module.
#[allow(unused_imports)]
use super::*;
use serde::{Deserialize, Serialize};
/// The persisted CLI config. JSON field names match the public tiny.place
/// (camelCase for the multi-word keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TinyplaceFileConfig {
    /// Optional tiny.place API endpoint override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub endpoint: Option<String>,
    /// Optional encoded identity secret.
    #[serde(rename = "secretKey", skip_serializing_if = "Option::is_none", default)]
    pub secret_key: Option<String>,
    /// Optional Sign-In With Solana authentication token.
    #[serde(rename = "siwsToken", skip_serializing_if = "Option::is_none", default)]
    pub siws_token: Option<String>,
    /// Optional OpenHuman owner identifier associated with the identity.
    #[serde(
        rename = "openHumanOwner",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub open_human_owner: Option<String>,
}
