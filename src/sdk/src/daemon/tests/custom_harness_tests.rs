//! Custom-harness selection, routing, and task-scoped override tests.

use std::sync::{Arc, Mutex as StdMutex};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::protocol::{HarnessProvider, TaskFrameKind};

use super::{base_config, conversation_runner, decoded_frames, recording_send, task_frame};

#[tokio::test]
async fn named_custom_harness_scopes_model_router_and_claude_tiers_to_one_run() {
    let observed = Arc::new(StdMutex::new(None));
    let capture = observed.clone();
    let run_task: RunTaskFn = Arc::new(move |options: RunTaskOptions| {
        let capture = capture.clone();
        Box::pin(async move {
            let endpoint = options
                .router
                .as_ref()
                .and_then(|router| router.base_url_for("claude"))
                .map(str::to_string);
            *capture.lock().unwrap() = Some((
                options.provider,
                options.model.clone(),
                endpoint,
                options.env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").cloned(),
            ));
            Ok(RunTaskResult {
                provider: options.provider,
                reply: "done".into(),
                events: 0,
                usage: None,
                session_id: None,
            })
        })
    });
    let mut config = base_config();
    config
        .env
        .insert("OPENROUTER_API_KEY".into(), "secret".into());
    config.custom_harnesses = vec![crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/pro | deepseek/fast | this-device",
    )
    .unwrap()];
    let (send, _) = recording_send();
    let runtime = DaemonRuntime::new(config, run_task, send);
    let mut frame = task_frame("custom", "work", None);
    frame.custom_harness = Some("deepseek".into());

    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    assert_eq!(
        observed.lock().unwrap().clone(),
        Some((
            HarnessProvider::Claude,
            Some("deepseek/pro".into()),
            Some(crate::config::OPENROUTER_ANTHROPIC_URL.into()),
            Some("deepseek/pro".into()),
        ))
    );
}

#[tokio::test]
async fn unknown_custom_harness_is_rejected_before_a_provider_runs() {
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        conversation_runner(Arc::new(StdMutex::new(Vec::new()))),
        send,
    );
    let mut frame = task_frame("custom", "work", None);
    frame.custom_harness = Some("missing".into());

    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(frames.iter().any(|frame| {
        frame.kind == TaskFrameKind::Error
            && frame
                .text
                .contains("custom harness \"missing\" is not configured")
    }));
}

#[tokio::test]
async fn explicit_provider_is_not_replaced_by_the_default_custom_harness() {
    let observed = Arc::new(StdMutex::new(None));
    let capture = observed.clone();
    let run_task: RunTaskFn = Arc::new(move |options: RunTaskOptions| {
        let capture = capture.clone();
        Box::pin(async move {
            *capture.lock().unwrap() = Some(options.provider);
            Ok(RunTaskResult {
                provider: options.provider,
                reply: "done".into(),
                events: 0,
                usage: None,
                session_id: None,
            })
        })
    });
    let mut config = base_config();
    config.providers.push(HarnessProvider::Codex);
    let mut default = crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/pro | | this-device",
    )
    .unwrap();
    default.default = true;
    config.custom_harnesses = vec![default];
    let (send, _) = recording_send();
    let runtime = DaemonRuntime::new(config, run_task, send);
    let mut frame = task_frame("explicit-provider", "work", None);
    frame.provider = Some(HarnessProvider::Codex);

    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    assert_eq!(*observed.lock().unwrap(), Some(HarnessProvider::Codex));
}

#[tokio::test]
async fn unavailable_default_custom_harness_does_not_replace_the_ordinary_provider() {
    let observed = Arc::new(StdMutex::new(None));
    let capture = observed.clone();
    let run_task: RunTaskFn = Arc::new(move |options: RunTaskOptions| {
        let capture = capture.clone();
        Box::pin(async move {
            *capture.lock().unwrap() = Some(options.provider);
            Ok(RunTaskResult {
                provider: options.provider,
                reply: "done".into(),
                events: 0,
                usage: None,
                session_id: None,
            })
        })
    });
    let mut config = base_config();
    let mut default = crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Codex | codex | deepseek/pro | | this-device",
    )
    .unwrap();
    default.default = true;
    config.custom_harnesses = vec![default];
    let (send, _) = recording_send();
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("untargeted", "work", None)),
    );
    runtime.idle().await;

    assert_eq!(*observed.lock().unwrap(), Some(HarnessProvider::Claude));
}
