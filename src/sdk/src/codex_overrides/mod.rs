//! Codex `-c` config overrides that make a routed Codex run actually reach a
//! non-OpenAI model.
//!
//! Pointing Codex at another endpoint with `OPENAI_BASE_URL` alone is not
//! enough, and the two ways it falls short are both silent-ish:
//!
//! - Codex 0.146 prefers a signed-in ChatGPT account over an API key, so a
//!   routed spawn still talks to OpenAI and answers with
//!   `The '<model>' model is not supported when using Codex with a ChatGPT
//!   account`. The endpoint override is simply not consulted.
//! - Codex refuses to describe a model it has no catalog entry for. It falls
//!   back to generic metadata, which declares the freeform `apply_patch` tool,
//!   `code_mode_only` and the search tool — all three sent on the wire as
//!   `custom` tools, which providers that accept only `function` tools reject
//!   with a 400 for the whole request.
//!
//! So a preset that opts in ([`crate::config::CustomHarnessConfig::codex_overrides`])
//! gets a full provider block, an API-key auth preference, and a model catalog
//! entry derived *at spawn time* from the catalog the installed Codex already
//! cached for itself — which is why the entry never goes stale across a Codex
//! upgrade, and why nothing is written to the operator's `~/.codex/config.toml`.
//!
//! Everything is passed as `-c key=value` argv, matching
//! [`crate::harness_hooks::codex`]: the operator's own Codex config, and their
//! `codex login`, stay untouched.
//!
//! The preset's knobs reach this module through the environment
//! ([`crate::config::CustomHarnessConfig::harness_env`]) rather than through a
//! parameter, because the environment is the one thing every spawn seam — the
//! interactive pane, the PTY executor, the headless daemon — already carries
//! from the preset to the child unchanged.

mod catalog;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

pub use catalog::{catalog_path, write_catalog};

use crate::protocol::HarnessProvider;

/// Set by a preset that wants this module's overrides. Anything but empty or
/// `0` enables them.
pub const OVERRIDES_ENV: &str = "MEDULLA_CODEX_OVERRIDES";
/// Reasoning effort declared to Codex (`model_reasoning_effort`).
pub const EFFORT_ENV: &str = "MEDULLA_CODEX_REASONING_EFFORT";
/// Context window, in tokens, declared to Codex for the routed model.
pub const CONTEXT_WINDOW_ENV: &str = "MEDULLA_CODEX_CONTEXT_WINDOW";
/// Human-readable name shown by Codex for the routed model.
pub const DISPLAY_NAME_ENV: &str = "MEDULLA_CODEX_DISPLAY_NAME";

/// The provider id the generated block is registered under.
///
/// Namespaced so it cannot collide with a provider the operator configured in
/// their own `~/.codex/config.toml`.
pub const PROVIDER_ID: &str = "medulla";

/// Effort used when the preset does not name one.
///
/// High rather than Codex's own medium: the presets this exists for are routed
/// at reasoning models whose vendors document high as the coding default.
pub const DEFAULT_EFFORT: &str = "high";

/// Context window declared when the preset does not name one.
///
/// Deliberately below what most routed models advertise. A model that claims a
/// million tokens loses accuracy long before it reaches them, and this number is
/// what every budget Codex computes — when to compact, how much history to keep
/// — is drawn against.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 300_000;

/// The `-c` argv a routed Codex spawn needs, or empty when this does not apply.
///
/// Returns nothing at all unless the provider is Codex, the preset opted in via
/// [`OVERRIDES_ENV`], and the router actually gave the child an endpoint — with
/// no endpoint there is nothing to point a provider block at, and overriding
/// `model_provider` would move an unrouted run off the operator's own account.
///
/// `model` is the slug the run was launched with (Codex's `-m`). It is what the
/// generated catalog entry is keyed on; without it there is no entry to write,
/// so the provider block is emitted alone.
///
/// # Errors
///
/// A catalog that cannot be derived is an error rather than a warning: the run
/// would otherwise reach the provider with tool shapes it rejects, and a 400 on
/// the first turn is far harder to read than a message naming the cache file.
pub fn launch_args(
    provider: HarnessProvider,
    model: Option<&str>,
    env: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    if provider != HarnessProvider::Codex || !enabled(env) {
        return Ok(Vec::new());
    }
    let Some(base_url) = env
        .get("OPENAI_BASE_URL")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };

    let mut args = Vec::new();
    push_override(&mut args, "model_provider", PROVIDER_ID);
    push_override(
        &mut args,
        &format!("model_providers.{PROVIDER_ID}.name"),
        "Medulla",
    );
    push_override(
        &mut args,
        &format!("model_providers.{PROVIDER_ID}.base_url"),
        base_url,
    );
    // The key itself is never on the command line: Codex resolves this variable
    // from the child environment the router already populated.
    push_override(
        &mut args,
        &format!("model_providers.{PROVIDER_ID}.env_key"),
        "OPENAI_API_KEY",
    );
    // Codex dropped the chat-completions wire in 0.146, and the endpoints a
    // preset routes to (OpenRouter, or Medulla's proxy in front of it) serve the
    // Responses API.
    push_override(
        &mut args,
        &format!("model_providers.{PROVIDER_ID}.wire_api"),
        "responses",
    );
    // Without this a signed-in ChatGPT account wins over the routed key and the
    // whole provider block is ignored.
    push_override(&mut args, "preferred_auth_method", "apikey");
    push_override(&mut args, "model_reasoning_effort", &effort(env));

    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return Ok(args);
    };
    let catalog = write_catalog(model, display_name(env, model), context_window(env), env)?;
    push_override(&mut args, "model_catalog_json", &catalog.to_string_lossy());
    Ok(args)
}

/// Whether the run's environment opts into these overrides.
fn enabled(env: &HashMap<String, String>) -> bool {
    env.get(OVERRIDES_ENV)
        .map(|value| value.trim())
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// The reasoning effort to declare, defaulting to [`DEFAULT_EFFORT`].
fn effort(env: &HashMap<String, String>) -> String {
    env.get(EFFORT_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_EFFORT)
        .to_string()
}

/// The context window to declare, defaulting to [`DEFAULT_CONTEXT_WINDOW`].
///
/// A non-numeric value is ignored rather than fatal: it is the operator's
/// config, and a wrong number should not stop the harness from starting.
fn context_window(env: &HashMap<String, String>) -> u32 {
    env.get(CONTEXT_WINDOW_ENV)
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|window| *window > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW)
}

/// The display name for the catalog entry, defaulting to the model slug.
fn display_name<'a>(env: &'a HashMap<String, String>, model: &'a str) -> &'a str {
    env.get(DISPLAY_NAME_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(model)
}

/// Append one `-c key=value` override as the two argv entries Codex expects.
///
/// Values are quoted as TOML strings: Codex parses the right-hand side as TOML,
/// and an unquoted OpenRouter slug (`deepseek/deepseek-v4-flash-0731`) is not a
/// valid bare value.
fn push_override(args: &mut Vec<String>, key: &str, value: &str) {
    args.push("-c".to_string());
    args.push(format!("{key}={}", toml_string(value)));
}

/// A TOML basic string, with the two characters that can break out escaped.
fn toml_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
