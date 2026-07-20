//! Process orchestration for a wrapped session: the [`run_wrapper`] entry point,
//! the [`run_wrapper_with`] core loop that drives the child CLI and the
//! tiny.place [`Bridge`](super::bridge::Bridge), and the exit-code / signal
//! plumbing around it.
//!
//! Spawning itself lives in [`child`], which hides whether the harness is on a
//! pseudo-terminal or on inherited stdio.

use std::collections::HashMap;
use std::time::Duration;

use crate::onboarding::OnboardingUi;
use crate::tinyplace::HarnessProvider;

use super::args::parse_wrapper_args;
use super::bridge::{
    build_bridge, drain_and_inject, mint_session_id, now_ms, provider_bin_env_key, pump_tailer,
    sync_harness_id,
};
use super::types::{PtySpawner, WrapperConfig, WrapperTimings};

mod child;

use child::{spawn_child, ChildSession};

/// How long to wait for the PTY reader to copy the child's final output before
/// restoring the terminal. Bounded so a wedged reader cannot hang the exit.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// The `medulla codex|claude|opencode` entry: build a [`WrapperConfig`] from the
/// process environment and run the wrapper, returning the child's exit code.
///
/// `onboarding_ui` is the interactive first-run screen the app injects on a TTY;
/// pass `None` to onboard headlessly. `pty_spawner` is the app-side seam that
/// runs the harness on a real pseudo-terminal when remote input is enabled;
/// pass `None` to keep the child on inherited stdio.
pub async fn run_wrapper(
    provider: HarnessProvider,
    args: &[String],
    onboarding_ui: Option<OnboardingUi>,
    pty_spawner: Option<PtySpawner>,
) -> anyhow::Result<i32> {
    let (no_bridge, child_args) = parse_wrapper_args(args);
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());

    // First-run worker registration (naming + owner). Skipped when running a plain
    // passthrough (no bridge). With an injected `onboarding_ui` this walks the
    // operator through onboarding; without one it auto-registers headlessly.
    // Aborting exits cleanly before launching the CLI.
    if !no_bridge
        && crate::onboarding::ensure_registered(&env, false, onboarding_ui)
            .await?
            .is_none()
    {
        return Ok(0);
    }

    run_wrapper_with(WrapperConfig {
        provider,
        child_args,
        env,
        cwd,
        no_bridge,
        session_id: None,
        pty_spawner,
    })
    .await
}

/// Run the wrapper described by `config`, returning the child's exit code.
pub async fn run_wrapper_with(mut config: WrapperConfig) -> anyhow::Result<i32> {
    use crate::tinyplace::env as tp_env;
    let bin = tp_env::provider_bin(config.provider, &config.env);
    let lookup = crate::daemon::providers::make_path_lookup(&config.env);
    if !lookup(&bin) {
        anyhow::bail!(
            "coding-agent CLI '{bin}' not found on PATH (install {} or set {})",
            config.provider.as_str(),
            provider_bin_env_key(config.provider),
        );
    }

    let start_ms = now_ms();
    let wrapper_session_id = config
        .session_id
        .clone()
        .unwrap_or_else(|| mint_session_id(config.provider));

    let timings = WrapperTimings::resolve(config.provider, &config.env);
    let mut bridge = build_bridge(&config, &wrapper_session_id, start_ms).await;
    let receive_active = bridge.as_ref().map(|b| b.receive_active).unwrap_or(false);

    // Extra args from `TINYPLACE_<P>_ARGS` are prepended to the child argv.
    let mut child_args = tp_env::provider_args(config.provider, &config.env);
    // Attribute commits made through this session to Medulla. Injected per-spawn,
    // so the operator's own `settings.json` is never touched.
    child_args.extend(crate::tinyplace::attribution::attribution_args(
        config.provider,
        &config.env,
    ));
    child_args.extend(config.child_args.iter().cloned());

    let ChildSession {
        input,
        mut done,
        mut kill,
        drained,
        restore,
    } = spawn_child(&bin, &child_args, &mut config, receive_active)?;

    if let Some(bridge) = bridge.as_mut() {
        bridge.lifecycle("session_start").await;
    }

    let mut tail_tick = tokio::time::interval(Duration::from_millis(timings.tail_poll_ms));
    let mut recv_tick = tokio::time::interval(Duration::from_millis(timings.receive_poll_ms));
    let mut status_tick =
        tokio::time::interval(Duration::from_millis(timings.status_throttle_ms as u64));
    let mut signal_fut = signal_future();

    let code = loop {
        tokio::select! {
            result = &mut done => {
                // A dropped sender means the waiter task died; report failure.
                break result.unwrap_or(1);
            }
            _ = tail_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    pump_tailer(bridge).await;
                }
            }
            _ = recv_tick.tick() => {
                if let (Some(bridge), Some(tx)) = (bridge.as_mut(), input.as_ref()) {
                    drain_and_inject(bridge, tx).await;
                }
            }
            _ = status_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    bridge.tick_status().await;
                }
            }
            _ = &mut signal_fut => {
                if let Some(kill_tx) = kill.take() {
                    let _ = kill_tx.send(());
                }
            }
        }
    };

    // Let the PTY reader flush whatever the child wrote on its way out, then put
    // the terminal back into cooked mode *before* any teardown logging, so those
    // messages land on a normal screen.
    if let Some(drained) = drained {
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, drained).await;
    }
    if let Some(restore) = restore {
        restore();
    }

    // Teardown: final transcript drain, then the closing lifecycle event.
    if let Some(bridge) = bridge.as_mut() {
        if let Some(mut tailer) = bridge.tailer.take() {
            let lines = tailer.drain();
            sync_harness_id(bridge);
            bridge.ingest_lines(lines).await;
        }
        bridge.lifecycle("session_end").await;
    }

    Ok(code)
}

/// A future that resolves on SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
///
/// On the PTY path the terminal is in raw mode, so Ctrl-C reaches the child's
/// own line discipline instead of us — exactly as if the harness had been run
/// directly. This future then only fires for signals sent to the wrapper itself.
fn signal_future() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(mut sigint), Ok(mut sigterm)) => Box::pin(async move {
                tokio::select! {
                    _ = sigint.recv() => {}
                    _ = sigterm.recv() => {}
                }
            }),
            _ => Box::pin(std::future::pending()),
        }
    }
    #[cfg(not(unix))]
    {
        Box::pin(async move {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}
