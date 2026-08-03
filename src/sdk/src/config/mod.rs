//! `medulla.tui.json`-compatible config — the subset the TUI reads, plus a
//! `backend` section for the HTTP runtime. Permissive: missing fields take
//! defaults, unknown fields are ignored.
//!
//! The module is split by responsibility: `urls` holds the endpoint base-URL
//! constants and their env-aware resolvers, `types` the config data model,
//! `load` the layered discovery/parse/merge that produces a [`LoadedConfig`],
//! `persist` writes back the few sections the app owns as state, and
//! `core_socket` resolves and validates the core (`medulla-serve`) socket path.
//! All public items are re-exported here so callers use `medulla::config::*`.

mod appearance;
mod core_socket;
mod custom_harnesses;
mod load;
mod persist;
mod types;
mod urls;

#[cfg(test)]
mod core_socket_tests;
#[cfg(test)]
mod custom_harnesses_tests;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod types_tests;
#[cfg(test)]
mod urls_tests;

pub use appearance::{AppearanceConfig, ResourceDisplay};
pub use core_socket::{validate_core_socket, CoreSocketError, CoreSocketSource};
pub use custom_harnesses::{
    load_custom_harnesses, load_layered_custom_harnesses, CustomHarnessConfig,
    OPENROUTER_ANTHROPIC_URL, OPENROUTER_API_KEY_ENV, OPENROUTER_OPENAI_URL,
};
pub use load::{default_link_config, explicit_config_from_env, load_config, CONFIG_PATH_ENV};
pub use persist::{
    clear_setting, persist_custom_harnesses, persist_host_workspaces, persist_hub_workers,
    persist_link_peers, persist_local_hosts, persist_root_setting, persist_routing_strategy,
    persist_section, persist_setting, persist_subscription_routing_strategy,
    persist_welcome_completed, persist_workflow_workspaces,
};
pub use types::{
    wire_value, AttributionConfig, BackendConfig, BudgetConfig, ControlStyle, CoreConfig,
    EvolveSettings, FieldPlacement, FieldVisibility, FleetConfig, HarnessNameStyle, HarnessSection,
    HostSection, HubSection, HubWorkerConfig, LinkConfig, LoadedConfig, McpSection, MedullaConfig,
    OnboardingConfig, OpencodeConfig, PathStyle, Peer, ProviderBudgetConfig, RouterConfig,
    RouterProviderConfig, StatusLineConfig, ThemeConfig, TuiConfig, UpdateConfig, WorkflowConfig,
    WorkflowsConfig, DEFAULT_CONTEXT_WINDOW_TOKENS,
};
pub use urls::{
    default_backend_base_url, default_tinyplace_base_url, display_host, is_staging,
    resolve_backend_base_url, resolve_tinyplace_base_url, PROD_BACKEND_BASE_URL,
    PROD_TINYPLACE_BASE_URL, STAGING_BACKEND_BASE_URL, STAGING_TINYPLACE_BASE_URL,
};
