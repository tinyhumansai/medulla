//! The harness dispatch that runs a workflow's `agent` nodes.
//!
//! A worker running a workflow already *is* a harness host, so a node's
//! instruction goes straight to the executor rather than back out over a bridge
//! to itself. This module resolves which provider and transport the node's
//! request runs on — mirroring the direct-task rules, with the embedded core
//! reachable by name — and collects the harness's transcript while it runs. On
//! success the transcript settles onto the node's step; on failure it rides the
//! error ([`RunError::WorkerWithTranscript`]) so the failed step keeps the
//! diagnostic trail its run view renders.

use std::sync::Arc;

use async_trait::async_trait;

use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::protocol::TokenUsage;

use super::super::super::providers::{Abort, RunTaskOptions};
use super::super::super::types::{DaemonRuntime, Inner};

/// Dispatch a workflow's `agent` nodes through this daemon's own executor.
///
/// A worker running a workflow already *is* a harness host, so a node's
/// instruction goes straight to the executor rather than back out over a bridge
/// to itself. The node's `agent_ref` names a provider hint when it matches one
/// this worker offers; otherwise the worker's default runs it.
pub(in crate::daemon) struct RuntimeDispatch {
    runtime: DaemonRuntime,
    /// The authenticated sender the workflow is being run for, so nodes inherit
    /// the same conversation attribution an ordinary task would get.
    conversation: String,
}

impl RuntimeDispatch {
    /// Build a dispatch attributed to one authenticated sender.
    pub(in crate::daemon) fn new(runtime: DaemonRuntime, conversation: String) -> Self {
        Self {
            runtime,
            conversation,
        }
    }

    /// The custom harness preset `request` names, resolved against this host.
    ///
    /// A workflow node reaches a harness through the same presets an ordinary
    /// task frame does, so this mirrors `handle_task`'s lookup rather than
    /// inventing a second rule: an explicitly named preset that this host has
    /// not configured is an error, and a request that states no preference at
    /// all inherits the operator's default preset when one is usable.
    ///
    /// Resolving it is what makes a preset more than a model string. The preset
    /// carries the endpoint, the API key name, and the harness's own knobs, and
    /// a dispatch that reads only [`TaskRequest::model`] sends a routed model
    /// slug to the harness's *default* account — which fails at the provider,
    /// far from the configuration that caused it.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Worker`] when `request` names a preset this host has
    /// no configuration for. Refused rather than silently downgraded to the
    /// default harness: a node that asked for a specific model and credentials
    /// must not quietly run on someone else's.
    fn preset(
        &self,
        request: &TaskRequest,
    ) -> Result<Option<crate::config::CustomHarnessConfig>, RunError> {
        let config = &self.runtime.inner.config;
        match request.custom_harness.as_deref() {
            Some(id) => config
                .custom_harnesses
                .iter()
                .find(|harness| harness.id == id)
                .cloned()
                .map(Some)
                .ok_or_else(|| {
                    RunError::Worker(format!(
                        "custom harness \"{id}\" is not configured on this host"
                    ))
                }),
            // Only when the node stated no preference of its own at all: a node
            // that named a plain provider asked for that provider, not for
            // whatever preset the operator happens to have marked default.
            None if request.provider.is_none() => Ok(config
                .custom_harnesses
                .iter()
                .find(|harness| harness.default && harness.key_present(&config.env))
                .cloned()),
            None => Ok(None),
        }
    }

    /// The provider and transport `request` will actually run on.
    ///
    /// An address hint may fall back for portability, but a named preset may
    /// not: its endpoint, credentials, and model only make sense with its
    /// base harness. The two callers that need the resolved pair — the
    /// dispatch itself and the run inspector's harness label — therefore share
    /// this rule.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Worker`] when a named preset's base provider is not
    /// offered by this daemon. Falling through to another provider would pair
    /// the preset's routing configuration with the wrong harness.
    fn resolve(
        &self,
        request: &TaskRequest,
        preset: Option<&crate::config::CustomHarnessConfig>,
    ) -> Result<
        (
            crate::protocol::HarnessProvider,
            crate::protocol::HarnessTransport,
        ),
        RunError,
    > {
        let inner = &self.runtime.inner;
        // A node that named the embedded core gets it, whether or not this
        // worker "offers" it. `config.providers` is the list of coding CLIs
        // found on PATH (see
        // [`crate::daemon::providers::detect_providers`]), and OpenHuman is
        // never on it — it has no binary to find. Falling through would send a
        // node that explicitly asked for the operator's own core to a coding
        // CLI instead, which is the one substitution that changes what the node
        // is for.
        //
        // Below the preset check rather than above it: a preset is a complete
        // description of one harness, so a node that named one has named
        // something more specific than a bare provider. In practice the two
        // cannot both be set — naming a preset leaves `provider` empty — and
        // this ordering is what keeps that true if they ever could.
        if preset.is_none()
            && (request.provider == Some(crate::protocol::HarnessProvider::Openhuman)
                || request.worker_address == crate::protocol::HarnessProvider::Openhuman.as_str())
        {
            return Ok((
                crate::protocol::HarnessProvider::Openhuman,
                crate::protocol::HarnessTransport::Cli,
            ));
        }
        // A preset outranks the address hint: it is a complete description of
        // one harness — binary, endpoint, credentials, model — and running it on
        // any other provider would pair its model with an account that cannot
        // serve it. A node may name a provider through its `agent_ref`;
        // anything this worker does not offer falls back to the default rather
        // than failing.
        let provider = preset
            .map(|harness| {
                self.runtime
                    .select_provider(Some(harness.base_harness))
                    .ok_or_else(|| unavailable_provider_error(inner, Some(harness.base_harness)))
            })
            .transpose()?
            .or_else(|| {
                crate::protocol::HarnessProvider::from_wire(&request.worker_address)
                    .filter(|p| inner.config.providers.contains(p))
            })
            .or_else(|| self.runtime.select_provider(request.provider))
            .unwrap_or(inner.config.default_provider);
        // Dropped when the provider fell back, because a transport the chosen
        // provider cannot speak is not a transport at all.
        let transport = request
            .transport
            .filter(|transport| transport.supported_by(provider))
            .unwrap_or_default();
        Ok((provider, transport))
    }
}

#[async_trait]
impl HarnessDispatch for RuntimeDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        let inner = &self.runtime.inner;
        let preset = self.preset(&request)?;
        let (provider, transport) = self.resolve(&request, preset.as_ref())?;

        // The preset's non-secret knobs ride down in the environment, which is
        // what every spawn seam hands the child unchanged — see
        // `crate::codex_overrides`, which reads them back there. Without this a
        // `codexOverrides` preset reaches Codex as a bare `-m <slug>` and Codex
        // asks its own default account for a model that account cannot serve.
        let mut run_env = inner.config.env.clone();
        if let Some(harness) = &preset {
            run_env.extend(harness.harness_env());
        }

        // Shared with the `on_event` callback below, which the executor owns
        // for the life of the run and drops before returning. A mutex rather
        // than a channel because the collector *is* the bound: a channel would
        // buffer everything a chatty node emits before anything applied a cap.
        let transcript = Arc::new(std::sync::Mutex::new(
            crate::harness_transcript::TranscriptCollector::new(),
        ));

        let options = RunTaskOptions {
            conversation: self.conversation.clone(),
            // A workflow node is discrete work, like the task frame that
            // started the graph — nodes share a conversation for attribution,
            // not a harness. Two nodes of one graph running in the same session
            // would let a later node read an earlier one's prompt as context.
            session_class: crate::sessions::SessionClass::Bounded,
            resume_session_id: None,
            workspace_context: Default::default(),
            provider,
            transport,
            prompt: request.instruction,
            cwd: inner.config.workspace.clone(),
            // Withheld, not merely unset: a node's harness is a *step* of a
            // graph that is already running. `workflow_run` would let it start
            // another one outside the loop bound, the approval gates, and the
            // concurrency budget the engine applies to its own nodes, and the
            // `fleet_*` verbs would let it dispatch into the very worker pool
            // this run is competing for. See [`crate::harness_tools`].
            //
            // Note what this replaces, and what follows from it. The ordinary
            // task path calls `with_tool_mode_at_depth`, which forces the ACP
            // transport whenever tools are wanted — ACP being the only way to
            // hand a harness an MCP server. Wanting no tools removes that
            // reason, so a node lands on the plain CLI spawn instead, and
            // `harness_hooks::launch_args` installs the operator's hooks onto
            // its argv for *every* provider. That is a real gain rather than a
            // neutral swap: ACP delivers hooks only to Claude, through its
            // session metadata, and Codex's ACP app-server still runs none
            // (see `crate::harness_hooks::acp`).
            //
            // `run_env`, not `config.env`: a node that selected a custom preset
            // needs that preset's non-secret knobs, which the spawn seam reads
            // back out of the environment.
            env: {
                let mut env = run_env;
                crate::harness_tools::withhold(&mut env);
                env.insert(
                    crate::control_socket::FLEET_DEPTH_ENV.to_string(),
                    request.fleet_depth.to_string(),
                );
                env
            },
            timeout_ms: inner.config.task_timeout_ms,
            // The preset's own model sits between the node's hint and this
            // daemon's pin: a node that selected a preset without naming a model
            // asked for that preset's model, not for whatever this host runs
            // when nobody states a preference.
            model: request
                .model
                .or_else(|| preset.as_ref().map(|harness| harness.model.clone()))
                .or_else(|| inner.config.model.clone()),
            agent: inner.config.agent.clone(),
            extra_args: inner.config.extra_args.clone(),
            skip_permissions: inner.config.skip_permissions,
            // The preset's endpoint and API-key name. The key itself is
            // resolved by name at the spawn seam, never inlined here.
            router: preset
                .as_ref()
                .map(crate::config::CustomHarnessConfig::router)
                .or_else(|| inner.config.router.clone()),
            attribution: inner.config.attribution,
            hooks: inner.config.hooks.clone(),
            abort: Abort::new(),
            // Collected, not forwarded. The run observer still reports progress
            // per node — nothing here emits a second status stream, which is
            // what the previous `None` was protecting against — but the events
            // are folded into a transcript that settles onto the run record.
            //
            // A node runs headless with nobody watching, so the reply used to
            // be all that survived it: the run view could say a step succeeded
            // and took four minutes without saying what happened in them. The
            // collector is bounded on the way in, so a chatty node costs a
            // fixed amount of memory rather than however much it emits.
            on_event: {
                let transcript = transcript.clone();
                Some(Box::new(move |event| {
                    if let Ok(mut collector) = transcript.lock() {
                        collector.observe(event);
                    }
                }))
            },
            on_stdin: None,
            on_session: None,
            on_workspace_context: None,
        };

        // Workflow nodes and detached evolution reviews run outside the inbound
        // task handler, but they are still harness sessions on this host. Share
        // its semaphore so a burst of failed workflows cannot exceed the
        // operator's configured concurrency.
        let _permit = inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        let result = match (inner.run_task)(options).await {
            Ok(result) => result,
            Err(message) => {
                // The harness failed after saying something: fold the
                // collector's account into the failure so a failed step keeps
                // the diagnostic trail its run view renders — the tool calls
                // and the error line that made the step fail. The executor has
                // returned, so it has dropped its `on_event` callback and this
                // is the only remaining handle; a poisoned lock (a panic inside
                // the callback) still loses the transcript, and that is not a
                // reason to report a less precise failure.
                let transcript = Arc::try_unwrap(transcript)
                    .ok()
                    .and_then(|lock| lock.into_inner().ok())
                    .map(crate::harness_transcript::TranscriptCollector::finish)
                    .unwrap_or_default();
                return Err(RunError::WorkerWithTranscript {
                    message,
                    transcript,
                });
            }
        };
        // The executor has returned, so it has dropped its `on_event` callback
        // and this is the only remaining handle — but a poisoned lock is still
        // possible (a panic inside the callback), and losing the transcript is
        // not a reason to fail a node that otherwise succeeded.
        let transcript = Arc::try_unwrap(transcript)
            .ok()
            .and_then(|lock| lock.into_inner().ok())
            .map(crate::harness_transcript::TranscriptCollector::finish)
            .unwrap_or_default();
        Ok(TaskOutcome {
            reply: result.reply,
            usage: result.usage.unwrap_or(TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            }),
            harness: Some(provider),
            session_id: None,
            transcript,
        })
    }

    /// The flavor this worker will really run the node on.
    ///
    /// `None` for a request naming a custom harness: this dispatch resolves
    /// providers, not custom harnesses, so the requested name remains the
    /// closest thing to the truth and overriding it would lose information.
    fn effective_harness(&self, request: &TaskRequest) -> Option<String> {
        if request.custom_harness.is_some() {
            return None;
        }
        // A preset can still be resolved here — the operator's default one, for
        // a request that named nothing. An unconfigured preset cannot reach this
        // point (the branch above returned), so the error case is unreachable
        // and reported as "no preset" rather than widening this signature.
        let preset = self.preset(request).ok().flatten();
        let (provider, transport) = self.resolve(request, preset.as_ref()).ok()?;
        Some(provider.flavor_name(transport).to_string())
    }
}

/// Match the task-frame handler's error when a requested provider is absent.
fn unavailable_provider_error(
    inner: &Inner,
    requested_provider: Option<crate::protocol::HarnessProvider>,
) -> RunError {
    let offered = inner
        .config
        .providers
        .iter()
        .map(|provider| provider.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let offered = if offered.is_empty() {
        "(none)".to_string()
    } else {
        offered
    };
    let requested = requested_provider
        .map(|provider| format!(" for requested \"{}\"", provider.as_str()))
        .unwrap_or_default();
    RunError::Worker(format!(
        "no available provider{requested}; daemon offers: {offered}"
    ))
}
