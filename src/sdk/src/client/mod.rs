//! HTTP/SSE client for the Medulla orchestration backend.
//!
//! Surfaces: auth (`/auth`), durable sessions (`/medulla/v1`), SSE event
//! streaming, one-shot orchestration (`/orchestration/v1`), and the public
//! feedback board (`/feedback`, in [`feedback`]).
//!
//! Every response is wrapped in a `{ "success": true, "data": ... }` envelope;
//! errors arrive as `{ "success": false, "error": ..., "errorCode": ... }` and
//! are surfaced as [`ClientError::Api`], preserving the `errorCode`.

pub mod error;
pub mod feedback;
pub mod sse;
pub mod types;

pub use error::{ClientError, Result};
pub use feedback::{
    FeedbackComment, FeedbackDetail, FeedbackGithub, FeedbackItem, FeedbackPage, FeedbackQuery,
    FeedbackSort, FeedbackStatus, FeedbackSubmission, FeedbackType,
};
pub use types::*;

use futures::stream::Stream;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

/// Default backend base URL.
pub const DEFAULT_BASE_URL: &str = "http://localhost:5000";

/// Client for the Medulla backend HTTP + SSE API.
#[derive(Debug, Clone)]
pub struct MedullaClient {
    base_url: String,
    jwt: String,
    http: reqwest::Client,
}

/// Builder for [`MedullaClient`].
#[derive(Debug, Default)]
pub struct MedullaClientBuilder {
    base_url: Option<String>,
    jwt: Option<String>,
    http: Option<reqwest::Client>,
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
        MedullaClient {
            base_url,
            jwt: self.jwt.unwrap_or_default(),
            http: self.http.unwrap_or_default(),
        }
    }
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
    pub fn set_jwt(&mut self, jwt: impl Into<String>) {
        self.jwt = jwt.into();
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.jwt),
        )
    }

    /// Send a request and unwrap the `{success, data}` envelope into `T`.
    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let resp = req.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        unwrap_envelope(status.as_u16(), &bytes)
    }

    // --- Auth ------------------------------------------------------------

    /// Exchange a one-time login token for a JWT
    /// (`POST /auth/login-token/consume`).
    pub async fn consume_login_token(&self, token: impl Into<String>) -> Result<String> {
        #[derive(serde::Serialize)]
        struct Body {
            token: String,
        }
        let req = self
            .http
            .post(self.url("/auth/login-token/consume"))
            .json(&Body {
                token: token.into(),
            });
        let out: LoginTokenResult = self.send(req).await?;
        Ok(out.jwt)
    }

    /// Fetch the authenticated principal (`GET /auth/me`).
    pub async fn me(&self) -> Result<Value> {
        let req = self.authed(self.http.get(self.url("/auth/me")));
        self.send(req).await
    }

    /// Account-level token usage for the active team/user
    /// (`GET /teams/me/usage`): cycle window, spend, per-model breakdown,
    /// remaining budget, plan. Returned as raw JSON — the shape is rendered
    /// defensively by the UI.
    pub async fn team_usage(&self) -> Result<Value> {
        let req = self.authed(self.http.get(self.url("/teams/me/usage")));
        self.send(req).await
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
        let part = reqwest::multipart::Part::text(content)
            .file_name("session.jsonl")
            .mime_str("application/x-ndjson")?;
        let form = reqwest::multipart::Form::new()
            .text("agent", agent.to_string())
            .part("file", part);

        let req = self.authed(
            self.http
                .post(self.url("/agent-integrations/history-rewards/uploads"))
                .multipart(form),
        );
        self.send(req).await
    }

    /// Score the uploaded history and grant the welcome credit
    /// (`POST /agent-integrations/history-rewards/claim`).
    ///
    /// Idempotent: a repeat call returns the same award with
    /// `already_claimed` set rather than granting again.
    pub async fn claim_history_reward(&self) -> Result<HistoryRewardClaim> {
        let req = self.authed(
            self.http
                .post(self.url("/agent-integrations/history-rewards/claim")),
        );
        self.send(req).await
    }

    /// Whether the caller has already earned the history reward
    /// (`GET /agent-integrations/history-rewards/status`).
    ///
    /// A user who has never uploaded gets a zeroed, unclaimed status rather
    /// than an error.
    pub async fn history_reward_status(&self) -> Result<HistoryRewardStatus> {
        let req = self.authed(
            self.http
                .get(self.url("/agent-integrations/history-rewards/status")),
        );
        self.send(req).await
    }

    // --- Sessions --------------------------------------------------------

    /// Create a durable session (`POST /medulla/v1/sessions`).
    pub async fn create_session(&self, title: Option<&str>) -> Result<SessionCreated> {
        #[derive(serde::Serialize)]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            title: Option<&'a str>,
        }
        let req = self
            .authed(self.http.post(self.url("/medulla/v1/sessions")))
            .json(&Body { title });
        self.send(req).await
    }

    /// List sessions (`GET /medulla/v1/sessions`).
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let req = self.authed(self.http.get(self.url("/medulla/v1/sessions")));
        self.send(req).await
    }

    /// Fetch a session's state (`GET /medulla/v1/sessions/:id`).
    pub async fn get_session(&self, session_id: &str) -> Result<SessionDetail> {
        let req = self.authed(
            self.http
                .get(self.url(&format!("/medulla/v1/sessions/{session_id}"))),
        );
        self.send(req).await
    }

    /// Archive a session (`DELETE /medulla/v1/sessions/:id`).
    pub async fn archive_session(&self, session_id: &str) -> Result<SessionArchived> {
        let req = self.authed(
            self.http
                .delete(self.url(&format!("/medulla/v1/sessions/{session_id}"))),
        );
        self.send(req).await
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
        #[derive(serde::Serialize)]
        struct Body<'a> {
            body: &'a str,
        }
        let sync_flag = if sync { "1" } else { "0" };
        let req = self
            .authed(
                self.http
                    .post(self.url(&format!("/medulla/v1/sessions/{session_id}/messages")))
                    .query(&[("sync", sync_flag)]),
            )
            .json(&Body { body });
        self.send(req).await
    }

    /// Replay messages after `after` (`GET .../messages?after=`).
    pub async fn list_messages(
        &self,
        session_id: &str,
        after: Option<i64>,
    ) -> Result<Vec<Message>> {
        let mut req = self
            .http
            .get(self.url(&format!("/medulla/v1/sessions/{session_id}/messages")));
        if let Some(after) = after {
            req = req.query(&[("after", after)]);
        }
        self.send(self.authed(req)).await
    }

    /// Replay events after `after` (`GET .../events?after=`).
    pub async fn list_events(
        &self,
        session_id: &str,
        after: Option<i64>,
    ) -> Result<Vec<EventEnvelope>> {
        let mut req = self
            .http
            .get(self.url(&format!("/medulla/v1/sessions/{session_id}/events")));
        if let Some(after) = after {
            req = req.query(&[("after", after)]);
        }
        self.send(self.authed(req)).await
    }

    /// Abort the running cycle (`POST /medulla/v1/sessions/:id/abort`).
    pub async fn abort(&self, session_id: &str) -> Result<AbortResult> {
        let req = self.authed(
            self.http
                .post(self.url(&format!("/medulla/v1/sessions/{session_id}/abort"))),
        );
        self.send(req).await
    }

    // --- SSE -------------------------------------------------------------

    /// Open a reconnecting SSE stream of events for a session
    /// (`GET /medulla/v1/sessions/:id/stream?token=<jwt>`).
    ///
    /// The returned stream auto-reconnects with `Last-Event-ID` and
    /// de-duplicates replayed frames by seq. Drop it to stop.
    pub fn stream_events(
        &self,
        session_id: &str,
        last_event_id: Option<u64>,
    ) -> impl Stream<Item = Result<EventEnvelope>> {
        let url = format!(
            "{}/medulla/v1/sessions/{}/stream?token={}",
            self.base_url,
            session_id,
            urlencode(&self.jwt),
        );
        sse::event_stream(self.http.clone(), url, last_event_id)
    }

    // --- Orchestration ---------------------------------------------------

    /// One-shot orchestration run (`POST /orchestration/v1/run`).
    ///
    /// Without tools the backend returns a final reply; with tools it returns
    /// the first [`LoopEvent`].
    pub async fn run(&self, input: &str, options: RunOptions) -> Result<RunResult> {
        #[derive(serde::Serialize)]
        struct Body<'a> {
            input: &'a str,
            #[serde(flatten)]
            options: &'a RunOptions,
        }
        let req = self
            .authed(self.http.post(self.url("/orchestration/v1/run")))
            .json(&Body {
                input,
                options: &options,
            });
        let value: Value = self.send(req).await?;
        parse_run_result(value)
    }

    /// Continue a tool-loop run (`POST /orchestration/v1/run/continue`).
    ///
    /// Pass an empty `tool_results` to poll a pending run.
    pub async fn continue_run(
        &self,
        cycle_id: &str,
        tool_results: Vec<ToolResult>,
    ) -> Result<LoopEvent> {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            cycle_id: &'a str,
            tool_results: Vec<ToolResult>,
        }
        let req = self
            .authed(self.http.post(self.url("/orchestration/v1/run/continue")))
            .json(&Body {
                cycle_id,
                tool_results,
            });
        self.send(req).await
    }
}

/// Decide whether a run response is a tool-less reply or a tool-loop event.
fn parse_run_result(value: Value) -> Result<RunResult> {
    if value.get("stop").is_some() {
        let ev: LoopEvent =
            serde_json::from_value(value).map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(RunResult::Loop(ev))
    } else {
        let reply: RunReply =
            serde_json::from_value(value).map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(RunResult::Reply(reply))
    }
}

/// Raw response envelope shared by every endpoint.
#[derive(Debug, Deserialize)]
struct RawEnvelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "errorCode", default)]
    error_code: Option<String>,
    #[serde(default)]
    details: Option<Value>,
}

/// Unwrap a `{success, data}` envelope into `T`, mapping failures and non-2xx
/// responses into [`ClientError::Api`].
pub(crate) fn unwrap_envelope<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<T> {
    let env: RawEnvelope = match serde_json::from_slice(body) {
        Ok(env) => env,
        Err(e) => {
            // Body was not a recognizable envelope. If the HTTP status already
            // signals failure, report that; otherwise it's a decode error.
            if !(200..300).contains(&status) {
                return Err(ClientError::Api {
                    status: Some(status),
                    message: String::from_utf8_lossy(body).trim().to_string(),
                    error_code: None,
                    details: None,
                });
            }
            return Err(ClientError::Decode(e.to_string()));
        }
    };

    if env.success && (200..300).contains(&status) {
        let data = env.data.unwrap_or(Value::Null);
        serde_json::from_value(data).map_err(|e| ClientError::Decode(e.to_string()))
    } else {
        Err(ClientError::Api {
            status: Some(status),
            message: env
                .error
                .unwrap_or_else(|| format!("request failed with status {status}")),
            error_code: env.error_code,
            details: env.details,
        })
    }
}

/// Minimal percent-encoding for the JWT query parameter and for untrusted path
/// segments (ids interpolated into a URL). Encodes everything outside the
/// unreserved set, so a `/` in an id cannot escape its segment.
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests;
