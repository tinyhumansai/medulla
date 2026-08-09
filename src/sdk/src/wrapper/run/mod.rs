//! Process orchestration for a wrapped session: the [`run_wrapper`] entry point,
//! the [`run_wrapper_with`] core loop that drives the child CLI and the
//! the [`Bridge`](super::bridge::Bridge), and the exit-code / signal
//! plumbing around it.
//!
//! Spawning itself lives in [`child`], which hides whether the harness is on a
//! pseudo-terminal or on inherited stdio.

use std::collections::HashMap;
use std::time::Duration;

use crate::onboarding::OnboardingUi;
use crate::protocol::HarnessProvider;

use super::args::parse_wrapper_args;
use super::bridge::{
    build_bridge, drain_and_inject, mint_session_id, now_ms, provider_bin_env_key, pump_tailer,
    sync_harness_id,
};
use super::types::{PtySpawner, WrapperConfig, WrapperTimings};

mod child;
mod types;

#[cfg(test)]
mod tests;

use child::{spawn_child, ChildSession};
use types::Signals;

/// How long to wait for the PTY reader to copy the child's final output before
/// restoring the terminal. Bounded so a wedged reader cannot hang the exit.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
/// Bound teardown even if the recipient remains backpressured.
const PUBLISH_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

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

    // Commit attribution is config-driven (`attribution.commit`, on by default).
    // A config that fails to load must not silently drop attribution, so fall
    // back to the same default the config type carries.
    // Attribution and hooks are both config-driven and both ride the same
    // `--settings` flag on Claude Code, so they are resolved together from one
    // load. A config that fails to load must not silently drop attribution, so
    // fall back to the same default the config type carries; hooks fall back to
    // none, since an unreadable config declares none.
    let (attribution, hooks) = crate::config::load_config(
        crate::config::explicit_config_from_env(&env),
        &env,
        std::path::Path::new(&cwd),
    )
    .map(|loaded| (loaded.config.attribution.commit, loaded.config.hooks))
    .unwrap_or((true, crate::harness_hooks::HooksConfig::default()));

    run_wrapper_with(WrapperConfig {
        provider,
        child_args,
        env,
        cwd,
        no_bridge,
        session_id: None,
        pty_spawner,
        attribution,
        hooks,
    })
    .await
}

/// Run the wrapper described by `config`, returning the child's exit code.
pub async fn run_wrapper_with(mut config: WrapperConfig) -> anyhow::Result<i32> {
    use crate::protocol::env as tp_env;
    // The embedded core's workspace is not this child's business — see
    // [`tp_env::CORE_STATE_VARS`] for what a coding harness that inherits it can
    // destroy. Dropped from both halves of the child's environment: this map is
    // its explicit overlay, and `spawn_child` removes the inherited value from
    // the child command without mutating this long-lived process.
    tp_env::scrub_core_state(&mut config.env, config.provider);
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

    // Extra args from `MEDULLA_<P>_ARGS` are prepended to the child argv.
    let mut child_args = tp_env::provider_args(config.provider, &config.env);
    if config.provider == crate::protocol::HarnessProvider::Claude
        && config
            .child_args
            .iter()
            .chain(child_args.iter())
            .any(|arg| arg == "--settings" || arg.starts_with("--settings="))
    {
        anyhow::bail!(
            "Claude --settings cannot be combined with Medulla attribution/hooks; move those settings into the normal Claude configuration"
        );
    }
    // Attribute commits made through this session to Medulla. Injected per-spawn,
    // so the operator's own `settings.json` is never touched.
    // Commit attribution and the operator's Medulla hooks share Claude Code's
    // single `--settings` flag, so they are built together rather than appended
    // independently — see `harness_hooks::launch_args`.
    let (launch_args, hook_notes) = crate::harness_hooks::launch_args(
        config.provider,
        config.attribution,
        &config.hooks,
        &config.env,
    );
    child_args.extend(launch_args);
    for note in &hook_notes {
        tracing::warn!(provider = config.provider.as_str(), "{note}");
    }
    // Every provider gets the prepare-commit-msg hook env vars; see the
    // attribution module docs for why the CLI flag alone is not enough.
    merge_attribution_env_into_config(&mut config);
    child_args.extend(config.child_args.iter().cloned());
    // The built-in reporting hooks just installed onto `child_args` need this
    // to find anything to report to — without it they spawn, find no grant,
    // and exit, on every one of `PostToolUse`'s per-tool-call firings for the
    // life of this wrapped session. The key is minted fresh rather than taken
    // from `wrapper_session_id`, which a caller may supply and reuse: two
    // concurrent runs sharing one key would share one grant, and the first
    // guard to drop would revoke the other run's reporting. The guard is
    // held for the rest of this function and revokes the grant once the
    // wrapped child has fully exited, rather than leaving it live for the
    // rest of this process.
    let hook_grant_session = format!("wrapper-{}", uuid::Uuid::new_v4());
    let _hook_grant = crate::harness_hooks::seed_hook_grant(hook_grant_session, &mut config.env);

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
    let mut signals = Signals::install();

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
            _ = signals.recv() => {
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
        bridge.finish_publications(PUBLISH_DRAIN_TIMEOUT).await;
    }

    Ok(code)
}

/// Merge the git-hook attribution env vars (for Codex / Opencode) into
/// `config.env`. Claude Code gets no additional env vars.
fn merge_attribution_env_into_config(config: &mut WrapperConfig) {
    let attribution_env = crate::attribution::attribution_env(config.attribution, &config.env);
    config.env.extend(attribution_env);
}

impl Signals {
    /// Install handlers for the host's termination signals.
    ///
    /// Each Unix stream registers independently, so a failure to install one
    /// signal does not cost the others: whichever succeeded is kept. Only when
    /// nothing could be installed does the source become [`Signals::Unavailable`],
    /// which never fires.
    pub(super) fn install() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let sigint = signal(SignalKind::interrupt()).ok();
            let sigterm = signal(SignalKind::terminate()).ok();
            if sigint.is_none() && sigterm.is_none() {
                Signals::Unavailable
            } else {
                Signals::Unix { sigint, sigterm }
            }
        }
        #[cfg(not(unix))]
        {
            Signals::CtrlC
        }
    }

    /// Resolve when a termination signal arrives.
    ///
    /// Cancel-safe, and safe to call repeatedly: each call awaits the live
    /// signal stream rather than resuming a future that already completed, so
    /// the run loop may drive it once per iteration for the whole session.
    pub(super) async fn recv(&mut self) {
        match self {
            #[cfg(unix)]
            Signals::Unix { sigint, sigterm } => {
                // Each branch polls only the streams that were installed by
                // [`Signals::install`]; a registration that failed (`None`)
                // awaits forever, so the branch never fires for a signal that
                // was not actually registered.
                tokio::select! {
                    _ = recv_signal(sigint.as_mut()) => {}
                    _ = recv_signal(sigterm.as_mut()) => {}
                }
            }
            #[cfg(not(unix))]
            Signals::CtrlC => {
                // A registration error is a disabled source, not an immediate
                // signal: awaiting it must not complete the branch, or the run
                // loop would treat the failed install as a Ctrl-C and kill the
                // child. Degrade to never resolving instead.
                if tokio::signal::ctrl_c().await.is_err() {
                    std::future::pending().await;
                }
            }
            Signals::Unavailable => std::future::pending().await,
        }
    }
}

/// Await the next arrival on `sig`, or forever if that stream was never
/// installed.
///
/// On Unix the two signals register independently, so a stream may be `None`
/// while its counterpart is live. A `None` stream must still be a valid
/// `select!` branch that simply never wins — it is not an immediate signal.
#[cfg(unix)]
async fn recv_signal(sig: Option<&mut tokio::signal::unix::Signal>) {
    if let Some(signal) = sig {
        let _ = signal.recv().await;
    } else {
        std::future::pending().await;
    }
}
