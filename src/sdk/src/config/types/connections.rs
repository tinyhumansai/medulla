//! Update, backend, core-socket, router, and budget configuration types.

use super::*;

/// The `update` section: the periodic release-update check. Disabled entirely
/// by `check = false` here, or by the `MEDULLA_NO_UPDATE_CHECK=1` environment
/// variable (see [`UpdateConfig::enabled`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateConfig {
    /// Whether the background TUI update check runs. Defaults to `true`.
    #[serde(default = "d_true")]
    pub check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { check: true }
    }
}

impl UpdateConfig {
    /// The effective on/off state: config `check` gated by the env kill-switch
    /// `MEDULLA_NO_UPDATE_CHECK` (any non-empty, non-`0` value disables it).
    pub fn enabled(&self, env: &HashMap<String, String>) -> bool {
        let killed = env
            .get("MEDULLA_NO_UPDATE_CHECK")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        self.check && !killed
    }
}

/// The optional `core` section: the NDJSON `medulla-serve` orchestration socket.
///
/// When configured, the core runtime attaches to a long-lived `medulla-serve`
/// process over a unix domain socket (the `medulla-serve` protocol, plan §2.2).
/// This milestone is attach-only: the socket must already be listening. The
/// section is unix-only; on Windows a request for it degrades to the
/// backend→mock chain (see [`super::load_config`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CoreConfig {
    /// Explicit NDJSON unix socket path. When unset the socket is resolved from
    /// `$XDG_RUNTIME_DIR/medulla/serve.sock`, then `<stateDir>/serve.sock` (see
    /// [`LoadedConfig::core_socket_path`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
}

/// Where the TUI reaches the Medulla backend HTTP API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackendConfig {
    /// Base URL of the Medulla backend HTTP API.
    #[serde(default = "d_backend_base")]
    pub base_url: String,
    /// Environment-variable name from which to resolve a bearer token.
    #[serde(default = "d_token_env")]
    pub token_env: String,
    /// Optional inline bearer token; environment-based credentials are preferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        BackendConfig {
            base_url: d_backend_base(),
            token_env: d_token_env(),
            token: None,
        }
    }
}

/// Per-provider router override. Only `baseUrl` is provider-scoped today: the
/// three harnesses reach a custom endpoint differently (claude needs an
/// Anthropic-passthrough URL, codex/opencode an OpenAI-compatible one), so the
/// endpoint can be steered per provider while the API key stays shared.
///
/// Matches the public router configuration contract so one config document
/// round-trips across clients and the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterProviderConfig {
    /// OpenAI-compatible (or Anthropic-passthrough) endpoint for this provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// The optional `[router]` section: a custom OpenAI-compatible router (gateway or
/// proxy) the worker points its harnesses at, so inference is centralized,
/// metered, and re-routable without hand-editing each harness's on-disk config.
///
/// camelCase on the wire (`baseUrl`, `apiKeyEnv`, `models`,
/// `providers.<p>.baseUrl`), matching the contract served at
/// `GET/PUT /medulla/v1/router`. Absent
/// entirely means the feature is off — zero behaviour change.
///
/// The API key is referenced by env-var **name** (`apiKeyEnv`), never inlined:
/// it is resolved from the daemon's own environment at spawn and excluded from
/// every frame and config diagnostic. Every field is optional; a config that
/// sets only `baseUrl` still routes, deferring model selection to the harness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterConfig {
    /// Top-level OpenAI-compatible endpoint for every provider without an override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Env-var NAME holding the API key — never the secret itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Tier → model/SKU mapping (`reasoning` / `compress` / `orchestrator`). A
    /// missing tier falls through to the harness's own configuration.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<String, String>,
    /// Per-provider overrides, keyed by provider id (`claude` / `codex` /
    /// `opencode`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, RouterProviderConfig>,
    /// Upstream serving providers the route is restricted to, by OpenRouter
    /// provider slug (`streamlake`, `novita`, …). Empty leaves the choice to
    /// OpenRouter.
    ///
    /// This is *upstream* routing, a different axis from `providers` above:
    /// that keys on which coding harness is being launched, this on which of
    /// OpenRouter's serving providers may answer the resulting request. It is
    /// honored only for OpenRouter-bound routes, where
    /// [`crate::inference_proxy`] turns it into the request body's `provider.only`
    /// — an endpoint override alone cannot express it, because the preference
    /// travels in the body rather than the URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_only: Vec<String>,
}

impl RouterConfig {
    /// The effective endpoint for `provider`, applying the documented precedence
    /// `providers.<p>.baseUrl` > top-level `baseUrl` > (unset → the harness's
    /// own on-disk config). Returns `None` when neither is configured.
    ///
    /// A blank endpoint (`baseUrl = ""`) at either level is treated as unset:
    /// a blank provider override cannot shadow a valid top-level endpoint, and a
    /// blank top-level value does not enable routing with an empty `*_BASE_URL`
    /// (which would break otherwise-normal spawns). Matches how blank `apiKeyEnv`
    /// values are filtered elsewhere.
    pub fn base_url_for(&self, provider: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|p| p.base_url.as_deref())
            .filter(|s| !s.is_empty())
            .or(self.base_url.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// The model/SKU mapped for `tier`, or `None` when the router leaves it to
    /// the harness.
    pub fn model_for_tier(&self, tier: &str) -> Option<&str> {
        self.models.get(tier).map(String::as_str)
    }
}

/// Operator-declared budget numbers for one provider in the `[budget]` section.
///
/// Mirrors the daemon's `ConfiguredBudget`: every field is optional, and when
/// present it makes the advertised `HarnessBudget` authoritative
/// (`source: configured`) instead of a best-effort estimate. camelCase on the
/// wire; `window` reuses the snake_case [`BudgetWindow`] values (`daily` /
/// `weekly` / `five_hour` / `unknown`) so one document round-trips across all
/// three modules.
///
/// No credential material lives here: `seat` is an opaque label only, never a
/// key or token, matching the frame contract that excludes secrets from budgets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderBudgetConfig {
    /// Opaque seat/subscription label the operator recorded (never a credential).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// The metering window the allowance renews on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<BudgetWindow>,
    /// The configured allowance for the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_tokens: Option<i64>,
    /// Consumption recorded so far in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<i64>,
    /// Remaining allowance, when the operator records it directly rather than
    /// leaving it to be derived from `limitTokens - usedTokens`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<i64>,
    /// Unix seconds until which the seat is parked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<i64>,
}

/// The optional `[budget]` section: operator-declared per-provider token budgets.
///
/// Absent entirely (the default) means every installed, usable harness advertises
/// a best-effort `estimate` with no invented numbers. A `[budget.providers.<p>]`
/// entry promotes that provider's advertised descriptor to `source: configured`
/// with the operator's exact numbers, so a hosted orchestrator can size tasks
/// against a real allowance. Keyed by provider id (`claude` / `codex` /
/// `opencode`), mirroring `[router.providers.<p>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BudgetConfig {
    /// Per-provider configured budgets, keyed by provider id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderBudgetConfig>,
}

impl BudgetConfig {
    /// The operator's configured numbers for `provider` (its wire id), if any.
    pub fn for_provider(&self, provider: &str) -> Option<&ProviderBudgetConfig> {
        self.providers.get(provider)
    }
}
