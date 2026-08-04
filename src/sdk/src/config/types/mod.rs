//! The config data model: every `[section]` the TUI reads, plus the
//! [`LoadedConfig`] result that pairs the parsed config with its provenance.
//!
//! Deserialization is permissive — missing fields take the `d_*` serde defaults
//! and unknown fields are ignored. Environment-dependent values (base URLs,
//! home-derived paths) are filled in afterwards by
//! [`load_config`](super::load_config), not here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::urls::{PROD_BACKEND_BASE_URL, PROD_TINYPLACE_BASE_URL};
use super::AppearanceConfig;
use crate::harness_hooks::HooksConfig;
use crate::protocol::{BudgetWindow, HarnessProvider};
use crate::runtime::fleet::{
    AgentTemplate, CapacitySnapshot, HarnessDescriptor, HostDescriptor, WorkspaceDescriptor,
};
use crate::runtime::{AgentDescriptor, RoutingStrategy, SubscriptionRoutingStrategy};

// --- serde default helpers -------------------------------------------------

fn d_state_dir() -> String {
    // Placeholder for `TuiConfig::default()` / bare deserialization; the real
    // value is `<medulla_home>/state`, filled in by `load_config`.
    "state".into()
}
fn d_forwarder_url() -> String {
    PROD_TINYPLACE_BASE_URL.into()
}
fn d_link_state_dir() -> String {
    // Placeholder; the real value is `<medulla_home>/link`, filled in by
    // `load_config`.
    "link".into()
}
fn d_opencode_cmd() -> String {
    "opencode".into()
}
fn d_agent() -> String {
    "build".into()
}
fn d_workspace() -> String {
    ".".into()
}
fn d_concurrency() -> u32 {
    4
}
fn d_true() -> bool {
    true
}
fn d_backend_base() -> String {
    PROD_BACKEND_BASE_URL.into()
}
fn d_token_env() -> String {
    "MEDULLA_TOKEN".into()
}
fn d_task_protocol() -> String {
    "task".into()
}

mod connections;
mod document;
mod fleet;
mod mcp;
#[cfg(test)]
mod mcp_tests;
mod orchestration;
mod presentation;
mod status_line;

pub use connections::*;
pub use document::*;
pub use fleet::*;
pub use mcp::*;
pub use orchestration::*;
pub use presentation::*;
pub use status_line::*;
