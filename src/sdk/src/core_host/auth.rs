//! Sign a booted core in, so its Medulla surface has a session to use.
//!
//! # Why this goes through `Core::raw`
//!
//! The typed embed facade models the Medulla surface and configuration; it does
//! not yet model auth. The controllers are registered — `DomainSet::embedded`
//! turns the `platform` family on — so the methods are live, they simply have
//! no typed wrapper upstream yet. `Core::raw` is the facade's documented escape
//! hatch for exactly that, and keeping every use of it in this one module means
//! the JSON and the stringly-typed method names are paid once: when a typed
//! `Auth` facade lands upstream, only this file changes.
//!
//! # Which token this is
//!
//! The core's *app session* JWT, not a Medulla-specific credential. The Medulla
//! orchestration API and the OpenHuman backend are one deployment, so the JWT
//! the login flow verifies against `/auth/me` is the same token
//! `medulla::resolve` looks for. Storing it here is what turns a signed-out core
//! into a usable runtime.

use openhuman_core::embed::{Core, CoreError};

/// RPC that validates a session JWT and writes it to the credential store.
///
/// Canonical `openhuman.<namespace>_<function>` spelling, as the dispatcher
/// expects it.
const STORE_SESSION: &str = "openhuman.auth_store_session";

/// RPC that reads the stored app session token back.
const GET_SESSION_TOKEN: &str = "openhuman.auth_get_session_token";

/// RPC that removes the stored app session.
const CLEAR_SESSION: &str = "openhuman.auth_clear_session";

/// RPC that reports who, if anyone, is signed in.
const GET_STATE: &str = "openhuman.auth_get_state";

/// Who the core is signed in as.
///
/// A narrow projection of the core's `AuthStateResponse`: the surfaces here
/// answer "signed in?" and "as whom?", and decoding only that keeps an upstream
/// addition to the response from breaking this host.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthState {
    /// Whether a usable session is stored.
    pub is_authenticated: bool,
    /// The signed-in account's id, when the core knows one.
    pub user_id: Option<String>,
}

/// Store `jwt` as the core's app session.
///
/// The core validates the token against the backend before persisting it, so a
/// rejected or expired JWT fails here rather than being written and failing on
/// every later call.
///
/// # Errors
///
/// [`CoreError::Rpc`] carrying the core's message when validation or the write
/// fails — including the case where the operator pasted a token for a different
/// deployment than the one the core resolves.
pub async fn store_session(core: &Core, jwt: &str) -> Result<(), CoreError> {
    let params = serde_json::json!({ "token": jwt });
    core.raw()
        .invoke(STORE_SESSION, params)
        .await
        .map_err(|message| CoreError::Rpc {
            method: STORE_SESSION,
            message,
        })?;
    tracing::debug!("[core_host] app session stored");
    Ok(())
}

/// The stored app session token, if there is one.
///
/// `Ok(None)` is the signed-out state, not a failure. Surfaces that need the JWT
/// itself — the hub uplink, the memory service's backend target —
/// take it from here rather than reading a file, so there is exactly one place a
/// session lives.
pub async fn session_token(core: &Core) -> Result<Option<String>, CoreError> {
    #[derive(serde::Deserialize)]
    struct TokenPayload {
        token: Option<String>,
    }
    let payload: TokenPayload = call(core, GET_SESSION_TOKEN, serde_json::json!({})).await?;
    Ok(payload.token.filter(|t| !t.trim().is_empty()))
}

/// Forget the stored app session.
///
/// Idempotent: clearing when already signed out succeeds. The core owns the
/// keychain entry and the profile file, so this is the only complete way to log
/// out — deleting a file next to it would leave the real credential behind.
pub async fn clear_session(core: &Core) -> Result<(), CoreError> {
    core.raw()
        .invoke(CLEAR_SESSION, serde_json::json!({}))
        .await
        .map_err(|message| CoreError::Rpc {
            method: CLEAR_SESSION,
            message,
        })?;
    tracing::debug!("[core_host] app session cleared");
    Ok(())
}

/// Who the core is signed in as, for the Account surface.
pub async fn state(core: &Core) -> Result<AuthState, CoreError> {
    call(core, GET_STATE, serde_json::json!({})).await
}

/// Dispatch `method` and decode the result, unwrapping the core's log envelope.
///
/// Controller handlers serialize through `RpcOutcome`, which emits the value
/// itself when the handler logged nothing and `{"result": …, "logs": […]}` when
/// it did. Whether a given method logs is an implementation detail that can
/// change without notice — the typed facade absorbs this for the methods it
/// models, and this does the same for the ones it does not.
async fn call<R: serde::de::DeserializeOwned>(
    core: &Core,
    method: &'static str,
    params: serde_json::Value,
) -> Result<R, CoreError> {
    let raw = core
        .raw()
        .invoke(method, params)
        .await
        .map_err(|message| CoreError::Rpc { method, message })?;
    serde_json::from_value(unwrap_envelope(raw))
        .map_err(|source| CoreError::Decode { method, source })
}

/// Strip the `{"result": …, "logs": […]}` wrapper when the payload has one.
///
/// Deliberately strict — exactly those two keys, with `logs` an array — so a
/// domain type that merely happens to carry a `result` field is not unwrapped
/// into its own contents.
pub(super) fn unwrap_envelope(raw: serde_json::Value) -> serde_json::Value {
    let is_envelope = raw.as_object().is_some_and(|map| {
        map.len() == 2
            && map.contains_key("result")
            // `.get`, not `map["logs"]`: indexing a `Map` panics on a missing
            // key, and a two-key object with `result` and something else is
            // exactly the shape this guard exists to reject.
            && map.get("logs").is_some_and(serde_json::Value::is_array)
    });
    if is_envelope {
        // Safe: `is_envelope` proved the shape.
        return raw
            .as_object()
            .and_then(|map| map.get("result").cloned())
            .unwrap_or(serde_json::Value::Null);
    }
    raw
}
