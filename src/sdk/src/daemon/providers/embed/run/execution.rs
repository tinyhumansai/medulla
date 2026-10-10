//! Resolve a Medulla route and drive one native OpenHuman turn.

use super::super::super::types::{RunTaskOptions, RunTaskOrigin, RunTaskResult};
use super::{watchdog, EventSink};
use crate::protocol::HarnessProvider;
use openhuman_embed::{Access, AgentDefinitionSpec, AgentSpec, AgentTurnOrigin, Provider, Route};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const PROGRESS_CAPACITY: usize = 512;

pub async fn run_local_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let RunTaskOptions {
        embed,
        budget,
        prompt,
        cwd,
        model,
        mut env,
        timeout_ms,
        abort,
        resume_session_id,
        router,
        on_event,
        on_session,
        on_stdin,
        hooks,
        origin,
        skip_permissions,
        ..
    } = options;
    if abort.is_aborted() {
        return Err("local task aborted before start".into());
    }
    let configured_headroom = embed.token_headroom();
    let headroom = match (
        budget.as_ref().map(|budget| budget.headroom_tokens),
        configured_headroom,
    ) {
        (Some(agent), Some(host)) => Some(agent.min(host)),
        (agent, host) => agent.or(host),
    };
    if headroom == Some(0)
        || budget
            .as_ref()
            .is_some_and(|budget| budget.exhausted || budget.headroom_tokens == 0)
    {
        return Err("Medulla worker token budget exhausted before start".into());
    }
    let model = super::super::effective_model(model, &env);
    let route = super::super::embedded_route(router.as_ref(), &env, model.as_deref())?
        .ok_or_else(|| "the local harness needs an inference route: name a router preset on the node, or set the preset's key variable in the environment".to_owned())?;
    let model = model.ok_or_else(|| "the local harness needs a model".to_owned())?;
    let thread_id =
        resume_session_id.unwrap_or_else(|| format!("medulla-{}", uuid::Uuid::new_v4()));
    let agent_id = format!(
        "worker-{}",
        &format!("{:x}", Sha256::digest(thread_id.as_bytes()))[..56]
    );
    let grant_session = format!("embed-{}", uuid::Uuid::new_v4());
    let home = crate::home::medulla_home(&env);
    let owned = embed.runtime(&home).await?;
    let _thread_claim = owned.turns.claim(&thread_id)?;
    let cwd = std::fs::canonicalize(&cwd).map_err(|error| format!("embed cwd: {error}"))?;
    // Agent homes retain session data; their generated skill copies are a
    // per-run policy snapshot and must not survive withholding or deletion.
    let skills = owned
        .runtime
        .workspace_dir()
        .join("agents")
        .join(&agent_id)
        .join("skills");
    match std::fs::remove_dir_all(&skills) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("native skill snapshot: {error}")),
    }
    let tool_env = super::super::environment::scrubbed(&env);
    let _hook_grant = crate::harness_hooks::seed_hook_grant(grant_session.clone(), &mut env);
    let hooks = super::super::hooks::Hooks::new(&hooks, env.clone(), cwd.clone());
    let access = match origin {
        RunTaskOrigin::Workflow => Access::full(),
        RunTaskOrigin::CapabilityProbe | RunTaskOrigin::UnattendedReview => Access::readonly(),
        _ => Access::full().origin(AgentTurnOrigin::Cli),
    };
    let mut spec = AgentSpec::new(&agent_id)
        .provider(Provider::routed(
            Route::openai_compatible(route.base_url, route.token)
                .header("X-Medulla-Session", &thread_id)
                .header("X-Medulla-Harness", "openhuman"),
        ))
        .model(model)
        .action_dir(&cwd)
        .access(access)
        .definition(AgentDefinitionSpec::new().max_iterations(40));
    if let Some(headroom) = headroom {
        spec = spec.stop_hook(Arc::new(super::super::budget::TokenBudget::new(
            headroom,
            embed.clone(),
        )));
    }
    let (native_tx, mut native_rx) = tokio::sync::mpsc::unbounded_channel();
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    let has_input = on_stdin.is_some();
    if let Some(register) = on_stdin {
        register(input_tx);
    } else {
        drop(input_tx);
    }
    if !skip_permissions && origin != RunTaskOrigin::Workflow && origin.has_operator_in_the_loop() {
        let approvals = super::super::permissions::Approvals::new(input_rx, native_tx.clone());
        spec = spec.can_use_tool(move |ctx| {
            let ctx = ctx.clone();
            let approvals = approvals.clone();
            Box::pin(async move {
                if !has_input {
                    return openhuman_embed::seams::ToolHookDecision::Deny(
                        "native tool approval needs an attached input surface".into(),
                    );
                }
                approvals.decide(ctx).await
            })
        });
    }
    spec = spec.tool_hook(hooks.clone());
    #[cfg(feature = "workflows")]
    if !crate::harness_tools::workflows_withheld(&env) {
        let skill_env = env.clone();
        let skill_cwd = cwd.clone();
        let skills = tokio::task::spawn_blocking(move || {
            use crate::workflows::skills::{managed_dir, skill_path, sync_managed, SkillTarget};
            sync_managed(SkillTarget::Generic, &skill_env, &skill_cwd)?;
            Ok::<_, std::io::Error>(
                skill_path(
                    SkillTarget::Generic,
                    &managed_dir(SkillTarget::Generic, &skill_env, &skill_cwd),
                    "placeholder",
                )
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_owned(),
            )
        })
        .await
        .map_err(|error| format!("native skills: {error}"))?
        .map_err(|error| format!("native skills: {error}"))?;
        if skills.is_dir() {
            spec = spec.skills_dir(skills);
        }
    }
    #[cfg(feature = "workflows")]
    let native_session = super::super::native_session::create(&env, &cwd, &grant_session).await?;
    let agent = owned
        .runtime
        .agent(spec)
        .map_err(|error| format!("embed agent: {error}"))?;
    if let Some(callback) = on_session {
        callback(thread_id.clone());
    }
    let mut sink = EventSink::new(on_event);
    sink.emit("user_prompt", json!({"text":prompt}));
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(PROGRESS_CAPACITY);
    let turn = agent
        .turn(prompt)
        .session(&thread_id)
        .cwd(&cwd)
        .tool_env(tool_env)
        .on_progress(progress_tx);
    #[cfg(feature = "workflows")]
    let turn = turn.tools(move |_| match &native_session {
        Some(session) => super::super::tools::belt(session.clone()),
        None => openhuman_embed::HostTurnTools::advertised(Vec::new()),
    });
    #[cfg(not(feature = "workflows"))]
    let turn = turn.tools(|_| openhuman_embed::HostTurnTools::advertised(Vec::new()));
    let mut turn = turn;
    let cancellation = turn.cancellation_handle();
    let outcome = watchdog::drive_events(
        turn.send(),
        &mut progress_rx,
        &mut native_rx,
        &abort,
        timeout_ms,
        &mut sink,
    )
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome.map_err(|error| format!("local turn failed: {error}")),
        Err(error) => {
            cancellation.cancel().await;
            Err(error)
        }
    };
    // No active turn remains when this returns, including after an idle timeout.
    if let Err(error) = owned.runtime.remove_agent(&agent_id).await {
        tracing::warn!(%agent_id, %error, "native agent cleanup failed");
    }
    #[cfg(feature = "workflows")]
    crate::mcp::attach::revoke_session(&grant_session);
    let turn = match outcome {
        Ok(turn) => turn,
        Err(error) => {
            sink.emit("error", json!({"message":error,"fatal":true}));
            return Err(error);
        }
    };
    let cleanup = openhuman_embed::process::CommandCleanup::default();
    let stop = cleanup
        .scope(async {
            tokio::select! {
                _ = abort.cancelled() => Err("local task aborted during Stop hook".to_owned()),
                result = hooks.stop(&thread_id, &turn.reply) => {
                    if let Err(error) = result {
                        tracing::warn!("native Stop hook: {error}");
                    }
                    Ok(())
                }
            }
        })
        .await;
    cleanup.wait().await;
    stop?;
    let reply = if turn.reply.trim().is_empty() {
        "The agent completed without a text response.".to_owned()
    } else {
        turn.reply
    };
    sink.emit("agent_message", json!({"text":reply}));
    Ok(RunTaskResult {
        provider: HarnessProvider::Openhuman,
        reply,
        events: sink.emitted(),
        usage: turn
            .usage
            .filter(|usage| usage.input_tokens > 0 || usage.output_tokens > 0)
            .map(|usage| crate::protocol::TokenUsage {
                input_tokens: usage.input_tokens.min(i64::MAX as u64) as i64,
                output_tokens: usage.output_tokens.min(i64::MAX as u64) as i64,
            }),
        session_id: Some(thread_id),
    })
}
