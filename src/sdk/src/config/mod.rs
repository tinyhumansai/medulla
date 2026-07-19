//! `medulla.tui.json`-compatible config — the subset the TUI reads, plus a
//! `backend` section for the HTTP runtime. Permissive: missing fields take
//! defaults, unknown fields are ignored.
//!
//! The module is split by responsibility: [`urls`] holds the endpoint base-URL
//! constants and their env-aware resolvers, [`types`] the config data model, and
//! [`load`] the layered discovery/parse/merge that produces a [`LoadedConfig`].
//! All public items are re-exported here so callers use `medulla::config::*`.

mod load;
mod types;
mod urls;

#[cfg(test)]
mod tests;

pub use load::load_config;
pub use types::{
    BackendConfig, CoreConfig, LoadedConfig, MedullaConfig, MemoryConfigSection, OpencodeConfig,
    Peer, ThemeConfig, TinyplaceConfig, TuiConfig, UpdateConfig,
};
pub use urls::{
    default_backend_base_url, default_tinyplace_base_url, is_staging, resolve_backend_base_url,
    resolve_tinyplace_base_url, PROD_BACKEND_BASE_URL, PROD_TINYPLACE_BASE_URL,
    STAGING_BACKEND_BASE_URL, STAGING_TINYPLACE_BASE_URL,
};
