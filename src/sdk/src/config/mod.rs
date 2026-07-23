//! `medulla.tui.json`-compatible config — the subset the TUI reads, plus a
//! `backend` section for the HTTP runtime. Permissive: missing fields take
//! defaults, unknown fields are ignored.
//!
//! The module is split by responsibility: [`urls`] holds the endpoint base-URL
//! constants and their env-aware resolvers, [`types`] the config data model,
//! [`load`] the layered discovery/parse/merge that produces a [`LoadedConfig`],
//! [`persist`] writes back the few sections the app owns as state, and
//! [`core_socket`] resolves and validates the core (`medulla-serve`) socket path.
//! All public items are re-exported here so callers use `medulla::config::*`.

mod core_socket;
mod load;
mod persist;
mod types;
mod urls;

#[cfg(test)]
mod core_socket_tests;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod types_tests;
#[cfg(test)]
mod urls_tests;

pub use core_socket::{validate_core_socket, CoreSocketError, CoreSocketSource};
pub use load::load_config;
pub use persist::{clear_setting, persist_setting, persist_welcome_completed};
pub use types::{
    BackendConfig, CoreConfig, LoadedConfig, MedullaConfig, MemoryConfigSection, OnboardingConfig,
    OpencodeConfig, Peer, ThemeConfig, TinyplaceConfig, TuiConfig, UpdateConfig, WorkflowConfig,
};
pub use urls::{
    default_backend_base_url, default_tinyplace_base_url, display_host, is_staging,
    resolve_backend_base_url, resolve_tinyplace_base_url, PROD_BACKEND_BASE_URL,
    PROD_TINYPLACE_BASE_URL, STAGING_BACKEND_BASE_URL, STAGING_TINYPLACE_BASE_URL,
};
