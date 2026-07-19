//! Process orchestration for a wrapped session: the [`run_wrapper`] entry point,
//! the [`run_wrapper_with`] core loop that spawns the child CLI and drives the
//! tiny.place [`Bridge`](super::bridge::Bridge), and the exit-code / signal
//! plumbing around it.

use std::collections::HashMap;
use std::io::Read as _;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::tinyplace::HarnessProvider;

use super::args::parse_wrapper_args;
use super::bridge::{
    build_bridge, drain_and_inject, mint_session_id, now_ms, provider_bin_env_key, pump_tailer,
    sync_harness_id,
};
use super::types::{WrapperConfig, WrapperTimings};

/// The `medulla codex|claude|opencode` entry: build a [`WrapperConfig`] from the
/// process environment and run the wrapper, returning the child's exit code.
pub async fn run_wrapper(provider: HarnessProvider, args: &[String]) -> anyhow::Result<i32> {
    let (no_bridge, child_args) = parse_wrapper_args(args);
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());

    // First-run worker registration (naming + owner). Skipped when running a plain
    // passthrough (no bridge). On a TTY this walks the operator through onboarding;
    // headless it auto-registers. Aborting exits cleanly before launching the CLI.
    if !no_bridge {
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        if crate::onboarding::ensure_registered(&env, is_tty, false)
            .await?
            .is_none()
        {
            return Ok(0);
        }
    }

    run_wrapper_with(WrapperConfig {
        provider,
        child_args,
        env,
        cwd,
        no_bridge,
        session_id: None,
    })
    .await
}

/// Run the wrapper described by `config`, returning the child's exit code.
pub async fn run_wrapper_with(config: WrapperConfig) -> anyhow::Result<i32> {
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
    child_args.extend(config.child_args.iter().cloned());

    // Spawn the child. stdout/stderr are always inherited (the user interacts with
    // the real CLI). stdin is piped only when we must inject input — otherwise it
    // is inherited so a full-screen TUI stays fully interactive.
    let mut command = Command::new(&bin);
    command
        .args(&child_args)
        .envs(&config.env)
        .current_dir(&config.cwd)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    if receive_active {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::inherit());
    }
    let mut child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("failed to start {bin}: {err}"))?;

    // Child stdin writer: a single task owns the pipe; injection and the raw
    // stdin pump feed it over a channel.
    let stdin_tx = if receive_active {
        child.stdin.take().map(|mut child_stdin| {
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            tokio::spawn(async move {
                while let Some(bytes) = rx.recv().await {
                    if child_stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = child_stdin.flush().await;
                }
            });
            tx
        })
    } else {
        None
    };
    // Forward the real terminal's stdin to the child (best-effort byte pump), only
    // when a TTY is attached so tests / pipes never consume the parent's stdin.
    if let Some(tx) = &stdin_tx {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut stdin = std::io::stdin();
                let mut buf = [0u8; 1024];
                loop {
                    match stdin.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    }

    if let Some(bridge) = bridge.as_mut() {
        bridge.lifecycle("session_start").await;
    }

    let mut tail_tick = tokio::time::interval(Duration::from_millis(timings.tail_poll_ms));
    let mut recv_tick = tokio::time::interval(Duration::from_millis(timings.receive_poll_ms));
    let mut status_tick =
        tokio::time::interval(Duration::from_millis(timings.status_throttle_ms as u64));
    let mut signal_fut = signal_future();

    let status = loop {
        tokio::select! {
            result = child.wait() => {
                break result;
            }
            _ = tail_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    pump_tailer(bridge).await;
                }
            }
            _ = recv_tick.tick() => {
                if let (Some(bridge), Some(tx)) = (bridge.as_mut(), stdin_tx.as_ref()) {
                    drain_and_inject(bridge, tx).await;
                }
            }
            _ = status_tick.tick() => {
                if let Some(bridge) = bridge.as_mut() {
                    bridge.tick_status().await;
                }
            }
            _ = &mut signal_fut => {
                let _ = child.start_kill();
            }
        }
    };

    // Teardown: final transcript drain, then the closing lifecycle event.
    if let Some(bridge) = bridge.as_mut() {
        if let Some(mut tailer) = bridge.tailer.take() {
            let lines = tailer.drain();
            sync_harness_id(bridge);
            bridge.ingest_lines(lines).await;
        }
        bridge.lifecycle("session_end").await;
    }

    let code = exit_code(status?);
    Ok(code)
}

/// Translate a child [`ExitStatus`](std::process::ExitStatus) into a shell-style
/// exit code (`128 + signal` for signal termination on Unix).
fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// A future that resolves on SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
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
