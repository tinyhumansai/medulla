//! Typed Medulla surface over the shared `tinyhumans-sdk` transport.
//!
//! Surfaces: auth (`/auth`), durable sessions (`/medulla/v1`), SSE event
//! streaming, one-shot orchestration (`/orchestration/v1`), and the public
//! feedback board (`/feedback`, in [`feedback`]).
//!
//! Every HTTP request is issued by [`tinyhumans_sdk::TinyHumansClient`], which
//! owns credential headers, the `{ "success": true, "data": ... }` envelope,
//! percent-encoding of path segments, and the not-exposed-route gate. Backend
//! failures arrive as [`ClientError::Api`] with the `errorCode` preserved — see
//! the conversion in [`error`].
//!
//! What remains here is the part the shared SDK does not model. Routes it
//! returns as open `DynamicResponse` JSON are decoded into the typed DTOs in
//! [`types`] and [`program`]; routes it now models itself are re-encoded into
//! those same DTOs by [`convert`], because the UI is written against them and a
//! few (task recurrence, dispatch state) are finer than the SDK's. On top of
//! that this module adds the reconnecting [`sse`] event stream, which the SDK
//! has no equivalent for.
//!
//! A few routes go through [`tinyhumans_sdk::TinyHumansClient::raw`], the SDK's
//! own escape hatch, because this crate models their contract more precisely
//! than the SDK's request types can express — a tri-state recurrence patch, the
//! `sync=1`/`sync=0` message flag, the NDJSON transcript upload. Each such call
//! says why inline. They still share the one transport.

pub mod error;
pub mod feedback;
pub mod program;
pub mod sse;
pub mod types;

pub use error::{ClientError, Result};
pub use feedback::{
    FeedbackComment, FeedbackDetail, FeedbackGithub, FeedbackItem, FeedbackPage, FeedbackQuery,
    FeedbackSort, FeedbackStatus, FeedbackSubmission, FeedbackType,
};
pub use program::*;
pub use types::*;

use futures::stream::Stream;
use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tinyhumans_sdk::api::medulla::{CreateSessionRequest, WorkspaceProfile};
use tinyhumans_sdk::api::orchestration::{RunRequest, RunResult as SdkRunResult};
use tinyhumans_sdk::api::types::{ContinueRunRequest, LoginTokenRequest};
use tinyhumans_sdk::{enc, QueryParam, TinyHumansClient};

/// Default backend base URL.
pub const DEFAULT_BASE_URL: &str = "http://localhost:5000";

/// How long the default HTTP client waits for a TCP/TLS connection.
const DEFAULT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long the default HTTP client tolerates a silent socket mid-response.
///
/// An idle timeout rather than a whole-request one: [`MedullaClient::stream_events`]
/// shares this client, and a total timeout would sever a healthy SSE stream on
/// schedule. Comfortably longer than the backend's `: ping` heartbeat, and long
/// enough for a slow model turn that has not yet emitted a byte.
const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Header through which the backend attributes traffic to a TinyHumans product.
const PRODUCT_IDENTITY_HEADER: &str = "x-sdk-name";

/// Product identity reported by this client for backend attribution.
const PRODUCT_IDENTITY: &str = "medulla";

/// The HTTP client used when the caller supplies none.
///
/// `reqwest::Client::default()` has no timeouts at all, so a black-holed
/// connection hangs sign-in — or any other call — forever. Falls back to the
/// untimed default only if the builder fails, which it does not for this
/// configuration.
fn default_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .read_timeout(DEFAULT_READ_TIMEOUT)
        .build()
        .unwrap_or_default()
}

impl MedullaClientBuilder {
    /// Set the backend base URL (default [`DEFAULT_BASE_URL`]).
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Set the bearer JWT sent with every request.
    pub fn jwt(mut self, jwt: impl Into<String>) -> Self {
        self.jwt = Some(jwt.into());
        self
    }

    /// Supply a preconfigured `reqwest::Client`.
    ///
    /// It is handed to the SDK as well, so ordinary requests and the SSE stream
    /// share one connection pool, TLS backend, and proxy policy.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Build the client.
    pub fn build(self) -> MedullaClient {
        let base_url = self
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let jwt = self.jwt.unwrap_or_default();
        let http = self.http.unwrap_or_else(default_http_client);
        let default_headers = product_identity_headers();
        MedullaClient {
            sdk: build_sdk(&base_url, &jwt, http.clone(), default_headers.clone()),
            base_url,
            jwt,
            http,
            default_headers,
        }
    }
}

/// Construct the SDK client that backs a [`MedullaClient`].
///
/// The host's `reqwest::Client` is passed in rather than letting the SDK build
/// its own, so both halves of this client share one transport policy.
fn build_sdk(
    base_url: &str,
    jwt: &str,
    http: reqwest::Client,
    default_headers: HeaderMap,
) -> TinyHumansClient {
    TinyHumansClient::new(base_url)
        .with_http_client(http)
        .with_default_headers(default_headers)
        .with_token(Some(jwt.to_string()))
}

/// Headers shared by the SDK transport and the hand-rolled SSE transport.
///
/// Constructed once with [`MedullaClient`], so every backend request reports
/// the same product identity regardless of its transport.
fn product_identity_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static(PRODUCT_IDENTITY_HEADER),
        HeaderValue::from_static(PRODUCT_IDENTITY),
    );
    headers
}

/// Decode an SDK response body into one of this crate's typed DTOs.
fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| ClientError::Decode(e.to_string()))
}

/// Serialize a request DTO whose wire shape this crate owns.
///
/// These types are plain data with infallible `Serialize` impls; the error arm
/// exists only because `serde_json` cannot say so in the type.
fn to_body<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| ClientError::Decode(e.to_string()))
}

/// Re-encode one of the SDK's typed models into this crate's own DTO.
///
/// The SDK now models the `/medulla/v1` and `/orchestration/v1` responses
/// itself, but this crate keeps its own DTOs: the TUI depends on them, and a
/// few carry fields (task recurrence, dispatch state) the SDK types do not. The
/// two sides agree on the wire shape, so a serde round-trip through the JSON
/// representation is the conversion — and it keeps the SDK's typed surface from
/// leaking into the UI layer.
fn convert<S: Serialize, T: DeserializeOwned>(value: S) -> Result<T> {
    decode(to_body(&value)?)
}

impl MedullaClient {
    /// Start building a client.
    pub fn builder() -> MedullaClientBuilder {
        MedullaClientBuilder::default()
    }

    /// Construct a client from a base URL and JWT.
    pub fn new(base_url: impl Into<String>, jwt: impl Into<String>) -> Self {
        Self::builder().base_url(base_url).jwt(jwt).build()
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The configured JWT.
    pub fn jwt(&self) -> &str {
        &self.jwt
    }

    /// Replace the JWT (e.g. after a token refresh).
    ///
    /// Rebuilds the SDK client, which captures the credential at construction.
    pub fn set_jwt(&mut self, jwt: impl Into<String>) {
        self.jwt = jwt.into();
        self.sdk = build_sdk(
            &self.base_url,
            &self.jwt,
            self.http.clone(),
            self.default_headers.clone(),
        );
    }

    /// Issue a request through the SDK's raw transport and decode the unwrapped
    /// payload.
    ///
    /// Used only where this crate's request or response contract is finer than
    /// the SDK's typed namespace method; every caller documents which.
    async fn raw<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[QueryParam],
        body: Option<&Value>,
    ) -> Result<T> {
        decode(self.sdk.raw().send(method, path, query, body, true).await?)
    }

    // --- Auth ------------------------------------------------------------

    /// Exchange a one-time login token for a JWT
    /// (`POST /auth/login-token/consume`).
    pub async fn consume_login_token(&self, token: impl Into<String>) -> Result<String> {
        let response = self
            .sdk
            .auth()
            .consume_login_token(&LoginTokenRequest {
                token: token.into(),
            })
            .await?;
        let out: LoginTokenResult = decode(response.0)?;
        Ok(out.jwt)
    }

    /// Fetch the authenticated principal (`GET /auth/me`).
    pub async fn me(&self) -> Result<Value> {
        Ok(self.sdk.auth().me().await?.0)
    }

    /// Account-level token usage for the active team/user
    /// (`GET /teams/me/usage`): cycle window, spend, per-model breakdown,
    /// remaining budget, plan. Returned as raw JSON — the shape is rendered
    /// defensively by the UI.
    pub async fn team_usage(&self) -> Result<Value> {
        Ok(self.sdk.teams().get_my_usage().await?.0)
    }

    // --- History rewards -------------------------------------------------

    /// Upload one redacted session transcript toward the onboarding reward
    /// (`POST /agent-integrations/history-rewards/uploads`).
    ///
    /// The caller is responsible for redacting `content` first — see
    /// [`crate::history_upload`]. The backend encrypts what it receives before
    /// storing it, and returns the claim's running metrics.
    ///
    /// Errors with [`ClientError::Api`] when the reward was already claimed, the
    /// session cap is reached, or the transcript exceeds the size limit.
    pub async fn upload_history_session(
        &self,
        agent: &str,
        content: String,
    ) -> Result<HistoryUploadResult> {
        // Raw multipart rather than `agent_integrations().upload_history`: that
        // helper sends the file part without a content type, and this upload
        // declares NDJSON.
        let part = reqwest::multipart::Part::text(content)
            .file_name("session.jsonl")
            .mime_str("application/x-ndjson")?;
        let form = reqwest::multipart::Form::new()
            .text("agent", agent.to_string())
            .part("file", part);

        decode(
            self.sdk
                .raw()
                .post_multipart("/agent-integrations/history-rewards/uploads", form)
                .await?,
        )
    }

    /// Score the uploaded history and grant the welcome credit
    /// (`POST /agent-integrations/history-rewards/claim`).
    ///
    /// Idempotent: a repeat call returns the same award with
    /// `already_claimed` set rather than granting again.
    pub async fn claim_history_reward(&self) -> Result<HistoryRewardClaim> {
        // Raw rather than `agent_integrations().claim_history_reward`: the SDK's
        // response type is a three-field projection, while the reveal screen
        // needs the settled status and the per-metric breakdown.
        self.raw(
            Method::POST,
            "/agent-integrations/history-rewards/claim",
            &[],
            None,
        )
        .await
    }

    /// Whether the caller has already earned the history reward
    /// (`GET /agent-integrations/history-rewards/status`).
    ///
    /// A user who has never uploaded gets a zeroed, unclaimed status rather
    /// than an error.
    pub async fn history_reward_status(&self) -> Result<HistoryRewardStatus> {
        // Raw for the same reason as `claim_history_reward`: the SDK's
        // `HistoryRewardsStatus` omits the tier, totals, and award this renders.
        self.raw(
            Method::GET,
            "/agent-integrations/history-rewards/status",
            &[],
            None,
        )
        .await
    }

    // --- Sessions --------------------------------------------------------

    /// Create a durable session (`POST /medulla/v1/sessions`).
    pub async fn create_session(&self, title: Option<&str>) -> Result<SessionCreated> {
        self.create_session_with(title, &[]).await
    }

    /// Create a durable session, attaching authored workspace profiles to the mint
    /// (`POST /medulla/v1/sessions` with `workspaceProfiles`).
    ///
    /// Each profile is one workspace root's verbatim `MEDULLA.md`, collected via
    /// [`crate::init::collect_profile_inputs`]. An empty slice mints a plain
    /// session (the `workspaceProfiles` key is omitted). The backend validates the
    /// shape and rejects a malformed profile with a 400.
    pub async fn create_session_with(
        &self,
        title: Option<&str>,
        workspace_profiles: &[WorkspaceProfileInput],
    ) -> Result<SessionCreated> {
        let request = CreateSessionRequest {
            title: title.map(str::to_owned),
            workspace_profiles: workspace_profiles
                .iter()
                .map(|profile| WorkspaceProfile {
                    workspace: profile.workspace.clone(),
                    medulla_md: profile.medulla_md.clone(),
                })
                .collect(),
            ..CreateSessionRequest::default()
        };
        convert(self.sdk.medulla().create_session(&request).await?)
    }

    /// List sessions (`GET /medulla/v1/sessions`).
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        convert(self.sdk.medulla().list_sessions(&[]).await?)
    }

    /// Fetch a session's state (`GET /medulla/v1/sessions/:id`).
    pub async fn get_session(&self, session_id: &str) -> Result<SessionDetail> {
        convert(self.sdk.medulla().get_session(session_id).await?)
    }

    /// Archive a session (`DELETE /medulla/v1/sessions/:id`).
    pub async fn archive_session(&self, session_id: &str) -> Result<SessionArchived> {
        convert(self.sdk.medulla().delete_session(session_id).await?)
    }

    /// Send a message (`POST /medulla/v1/sessions/:id/messages`).
    ///
    /// With `sync = false` the backend returns 202 `{cycleId, seq}`; with
    /// `sync = true` it blocks and returns `{cycleId, seq, reply}`.
    pub async fn send_message(
        &self,
        session_id: &str,
        body: &str,
        sync: bool,
    ) -> Result<SendResult> {
        // Raw rather than `medulla().send_session_message`: that helper spells
        // the flag `sync=true`/`sync=false`, and this contract is `1`/`0`.
        let sync_flag = if sync { "1" } else { "0" };
        self.raw(
            Method::POST,
            &format!("/medulla/v1/sessions/{}/messages", enc(session_id)),
            &[("sync", Some(sync_flag.to_string()))],
            Some(&serde_json::json!({ "body": body })),
        )
        .await
    }

    /// Replay messages after `after` (`GET .../messages?after=`).
    pub async fn list_messages(
        &self,
        session_id: &str,
        after: Option<i64>,
    ) -> Result<Vec<Message>> {
        let query: [QueryParam; 1] = [("after", after.map(|a| a.to_string()))];
        convert(
            self.sdk
                .medulla()
                .session_messages(session_id, &query)
                .await?,
        )
    }

    /// Replay events after `after` (`GET .../events?after=`).
    pub async fn list_events(
        &self,
        session_id: &str,
        after: Option<i64>,
    ) -> Result<Vec<EventEnvelope>> {
        let query: [QueryParam; 1] = [("after", after.map(|a| a.to_string()))];
        convert(
            self.sdk
                .medulla()
                .session_events(session_id, &query)
                .await?,
        )
    }

    /// Abort the running cycle (`POST /medulla/v1/sessions/:id/abort`).
    pub async fn abort(&self, session_id: &str) -> Result<AbortResult> {
        convert(self.sdk.medulla().abort_session(session_id).await?)
    }

    /// Read the backend's configured worker routing strategy
    /// (`GET /medulla/v1/routing/strategy`).
    ///
    /// Returns `None` when the backend has no strategy configured or the value is
    /// unrecognized, so an absent/garbled strategy degrades to "no backend
    /// preference" rather than an error — the operator's local config still wins.
    pub async fn get_routing_strategy(&self) -> Result<Option<crate::runtime::RoutingStrategy>> {
        let value = self.sdk.medulla().routing_strategy().await?;
        Ok(value
            .strategy
            .as_deref()
            .and_then(crate::runtime::RoutingStrategy::from_wire))
    }

    /// Persist the operator's worker routing strategy to the backend
    /// (`PUT /medulla/v1/routing/strategy`) as `{ "strategy": <camelCase> }`.
    pub async fn set_routing_strategy(
        &self,
        strategy: crate::runtime::RoutingStrategy,
    ) -> Result<()> {
        self.sdk
            .medulla()
            .set_routing_strategy(strategy.as_wire())
            .await?;
        Ok(())
    }

    /// Read the connected worker roster (`GET /medulla/v1/roster`).
    pub async fn roster(&self) -> Result<Vec<RosterWorker>> {
        convert(self.sdk.medulla().roster().await?)
    }

    /// List the operator-owned program task ledger (`GET /medulla/v1/tasks`).
    pub async fn list_program_tasks(&self) -> Result<Vec<ProgramTask>> {
        convert(self.sdk.medulla().list_tasks(&[]).await?)
    }

    /// Create an operator-owned program task (`POST /medulla/v1/tasks`).
    pub async fn create_program_task(&self, input: CreateProgramTask) -> Result<ProgramTask> {
        // Raw rather than `medulla().create_task`: the SDK types `recurrence` as
        // open JSON, while [`TaskRecurrence`] models the rule.
        let payload: TaskPayload = self
            .raw(
                Method::POST,
                "/medulla/v1/tasks",
                &[],
                Some(&to_body(&input)?),
            )
            .await?;
        Ok(payload.task)
    }

    /// Update an operator-owned program task (`PATCH /medulla/v1/tasks/:id`).
    pub async fn update_program_task(
        &self,
        task_id: &str,
        patch: UpdateProgramTask,
    ) -> Result<ProgramTask> {
        // Raw rather than `medulla().update_task`: [`UpdateProgramTask`]
        // distinguishes an omitted `recurrence` from an explicit `null` that
        // clears it, which the SDK's `Option<Value>` field cannot express — it
        // would silently drop the clear.
        let payload: TaskPayload = self
            .raw(
                Method::PATCH,
                &format!("/medulla/v1/tasks/{}", enc(task_id)),
                &[],
                Some(&to_body(&patch)?),
            )
            .await?;
        Ok(payload.task)
    }

    /// Delete an operator-owned program task (`DELETE /medulla/v1/tasks/:id`).
    pub async fn delete_program_task(&self, task_id: &str) -> Result<bool> {
        Ok(self.sdk.medulla().delete_task(task_id).await?)
    }

    /// List configured GitHub task sources (`GET /medulla/v1/tasks/sources`).
    pub async fn list_program_task_sources(&self) -> Result<Vec<ProgramTaskSource>> {
        convert(self.sdk.medulla().list_task_sources().await?)
    }

    /// Configure a GitHub task source (`POST /medulla/v1/tasks/sources`).
    ///
    /// `input.token` is write-only: the backend encrypts it and responses expose
    /// only [`ProgramTaskSource::has_token`].
    pub async fn create_program_task_source(
        &self,
        input: CreateProgramTaskSource,
    ) -> Result<ProgramTaskSource> {
        // Raw rather than `medulla().create_task_source`: this request types
        // `state` as [`GithubIssueState`] and omits `labels` only when unset,
        // where the SDK takes an open string and drops an explicitly empty list.
        let payload: TaskSourcePayload = self
            .raw(
                Method::POST,
                "/medulla/v1/tasks/sources",
                &[],
                Some(&to_body(&input)?),
            )
            .await?;
        Ok(payload.source)
    }

    /// Synchronize one GitHub source into the task ledger.
    pub async fn sync_program_task_source(&self, source_id: &str) -> Result<TaskSourceSyncResult> {
        convert(self.sdk.medulla().sync_task_source(source_id).await?)
    }

    /// Remove a configured GitHub task source.
    pub async fn delete_program_task_source(&self, source_id: &str) -> Result<bool> {
        Ok(self.sdk.medulla().delete_task_source(source_id).await?)
    }

    // --- SSE -------------------------------------------------------------

    /// Open a reconnecting SSE stream of events for a session
    /// (`GET /medulla/v1/sessions/:id/stream?token=<jwt>`).
    ///
    /// The returned stream auto-reconnects with `Last-Event-ID` and
    /// de-duplicates replayed frames by seq. Drop it to stop.
    ///
    /// Built by hand rather than through the SDK: the SDK's transport buffers a
    /// whole response body before returning, which a stream that never ends
    /// cannot use. The `reqwest::Client` is the one the SDK was given, so the
    /// two still share a connection pool.
    pub fn stream_events(
        &self,
        session_id: &str,
        last_event_id: Option<u64>,
    ) -> impl Stream<Item = Result<EventEnvelope>> {
        let url = format!(
            "{}/medulla/v1/sessions/{}/stream?token={}",
            self.base_url,
            enc(session_id),
            enc(&self.jwt),
        );
        sse::event_stream(
            self.http.clone(),
            self.default_headers.clone(),
            url,
            last_event_id,
        )
    }

    // --- Orchestration ---------------------------------------------------

    /// One-shot orchestration run (`POST /orchestration/v1/run`).
    ///
    /// Without tools the backend returns a final reply; with tools it returns
    /// the first [`LoopEvent`].
    pub async fn run(&self, input: &str, options: RunOptions) -> Result<RunResult> {
        let request = RunRequest {
            input: input.to_string(),
            session_id: options.session_id.clone(),
            // Selected by the hosted flavours, not by this client.
            flavor: None,
            tools: match &options.tools {
                Some(tools) => tools.iter().map(convert).collect::<Result<Vec<_>>>()?,
                None => Vec::new(),
            },
            options: options.options.as_ref().map(convert).transpose()?,
        };
        match self.sdk.orchestration().run(&request).await? {
            SdkRunResult::Reply(reply) => Ok(RunResult::Reply(convert(*reply)?)),
            SdkRunResult::Loop(event) => Ok(RunResult::Loop(convert(*event)?)),
        }
    }

    /// Continue a tool-loop run (`POST /orchestration/v1/run/continue`).
    ///
    /// Pass an empty `tool_results` to poll a pending run.
    pub async fn continue_run(
        &self,
        cycle_id: &str,
        tool_results: Vec<ToolResult>,
    ) -> Result<LoopEvent> {
        let request = ContinueRunRequest {
            cycle_id: cycle_id.to_string(),
            tool_results: tool_results
                .iter()
                .map(convert)
                .collect::<Result<Vec<_>>>()?,
        };
        convert(self.sdk.orchestration().continue_run(&request).await?)
    }
}

#[cfg(test)]
mod tests;
