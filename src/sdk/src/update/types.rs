//! The update data model: the release `latest.json` manifest, its per-platform
//! asset entries, and the resolved [`UpdateInfo`] a check produces.
//!
//! These types are pure data with derive-only impls; the parsing, version
//! comparison, and install logic that consume them live in the sibling
//! [`check`](super::check) and [`install`](super::install) modules.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The default manifest URL: the "latest" release always redirects here.
pub const DEFAULT_UPDATE_URL: &str =
    "https://github.com/tinyhumansai/medulla/releases/latest/download/latest.json";

/// One platform's downloadable asset in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub url: String,
    pub sha256: String,
}

/// The `latest.json` release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub platforms: HashMap<String, PlatformEntry>,
}

/// A resolved, actionable update for the running platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
}
