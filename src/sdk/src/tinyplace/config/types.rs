//! Data types for the `config` module.
#[allow(unused_imports)]
use super::*;
use serde::{Deserialize, Serialize};
/// The persisted CLI config. JSON field names match the TypeScript SDK
/// (camelCase for the multi-word keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TinyplaceFileConfig {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub endpoint: Option<String>,
    #[serde(rename = "secretKey", skip_serializing_if = "Option::is_none", default)]
    pub secret_key: Option<String>,
    #[serde(rename = "siwsToken", skip_serializing_if = "Option::is_none", default)]
    pub siws_token: Option<String>,
    #[serde(
        rename = "openHumanOwner",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub open_human_owner: Option<String>,
}
