//! `medulla remote <host> --exec <command>`: run one command on a remote host
//! and print what its screen showed.
//!
//! The smallest thing that exercises every link in the chain — SSH bootstrap,
//! identity enrollment, direct UDP link, session open, keystrokes up, frames
//! down, screen folded — and the one that makes the chain testable from a shell
//! script rather than only from Rust. The Docker suite is built on it.
//!
//! It is also genuinely useful on its own: "what does `git status` say on the
//! build box" is a question worth one command.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use medulla::config::RemoteHostSection;

use super::{task, RemoteHost};

/// How long to wait for the command's output to appear on the screen.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for the host to answer a request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to stay connected waiting for a close to be acknowledged.
///
/// Shorter than the others on purpose: the command has already produced its
/// output by this point, and the only thing still outstanding is tidying up.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run `command` on `host_id` and print the resulting screen.
///
/// # Errors
///
/// Anything that stops the chain: an unknown host id, a failed bootstrap (whose
/// message names the specific cause), a host that offers no shell, or a session
/// that never produced output.
pub async fn run(args: &[String]) -> anyhow::Result<()> {
    let host_id = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("usage: medulla remote <host> --exec <command>"))?;
    // Optional when a harness is named: `--harness claude` on its own is "open a
    // session there and show me what it says", which is a complete request. A
    // shell needs a command, because a shell with nothing to run has nothing to
    // report.
    let command = flag(args, "--exec").unwrap_or_default();
    if command.trim().is_empty() && flag(args, "--harness").is_none() {
        anyhow::bail!("--exec <command> is required, or name a --harness to open");
    }

    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()?;
    let loaded = medulla::config::load_config(flag(args, "--config").as_deref(), &env, &cwd)?;
    // Through the shared resolver, which applies the same `enabled` filter and
    // id dedupe the picker offers from. Searching the raw list would also have
    // passed a fixed index to `remote_host_id`, so every entry needing the
    // positional fallback would have resolved to the same id.
    let section = medulla::config::remote_host_section(&loaded.config.remote_hosts, &host_id)
        .or_else(|| {
            // Also accept the bare SSH destination, so `medulla remote
            // tower.local` works without looking up what it was named.
            medulla::config::remote_hosts(&loaded.config.remote_hosts)
                .iter()
                .find(|host| host.host == host_id)
                .and_then(|host| {
                    medulla::config::remote_host_section(&loaded.config.remote_hosts, &host.id)
                })
        })
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no [[remoteHosts]] entry called {host_id}; known hosts: {}",
                known(&loaded.config.remote_hosts)
            )
        })?;

    let home = medulla::home::medulla_home(&env);
    // One identity directory per host, because the link takes an advisory lock
    // on it for its lifetime: hosts sharing a directory would leave all but the
    // first unable to connect.
    // A distinct identity from the TUI's, and deliberately not a unique one.
    //
    // `RemoteHost::connect` holds the directory's `node.lock` for the link's
    // life, so sharing the TUI's would make this command fail with a busy
    // identity whenever the TUI had that host open — which is most of the time
    // it is useful. A *fresh* directory per invocation would avoid that too, but
    // each new client id enrolls a new daemon on the far side, so every `--exec`
    // would strand another one; a single stable `-exec` identity reuses one.
    let state_dir = task::client_state_dir(
        &home,
        &format!("{}-exec", medulla::config::remote_host_id(&section, 0)),
    );

    let workspace = flag(args, "--workspace").unwrap_or_else(|| section.workspace.clone());
    eprintln!("connecting to {host_id}…");
    let mut host = RemoteHost::connect(&host_id, &section, &state_dir, &workspace)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    host.hello().await.map_err(|e| anyhow::anyhow!(e))?;
    // Waits for the answer, not for it to be non-empty: a host with nothing
    // installed answers with an empty list and is still connected.
    await_for("the host to answer", REPLY_TIMEOUT, &mut host, |host| {
        !host.capabilities().version.is_empty()
    })
    .await?;

    // `--harness` names one of the host's own advertised ids; without it a shell
    // is chosen, because that is what "run this command over there" means.
    let wanted = flag(args, "--harness");
    let offered = host.capabilities().harnesses.clone();
    let shell = match wanted.as_deref() {
        Some(wanted) => offered
            .iter()
            .find(|choice| choice.id == wanted || choice.provider == wanted)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{host_id} does not offer {wanted}; it offers: {}",
                    offered
                        .iter()
                        .map(|choice| choice.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?
            .id
            .clone(),
        None => offered
            .iter()
            .find(|choice| choice.provider == "shell")
            .ok_or_else(|| anyhow::anyhow!("{host_id} offers no shell"))?
            .id
            .clone(),
    };

    host.open("exec", &shell, &workspace, 120, 40, None)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut session_id = String::new();
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Instant::now() < deadline && session_id.is_empty() {
        for event in host.pump().await {
            match event {
                // Already waited for above; nothing to do a second time.
                super::PumpEvent::Connected => {}
                super::PumpEvent::Opened(id) => session_id = id,
                // Reported immediately rather than left to time out: the host
                // already said exactly why, and waiting out REPLY_TIMEOUT to
                // report only "never confirmed the session" would throw that
                // reason away.
                super::PumpEvent::OpenFailed(reason) => {
                    anyhow::bail!("{host_id} could not start it: {reason}");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if session_id.is_empty() {
        anyhow::bail!("{host_id} never confirmed the session");
    }

    // From here on the far side has a live session that must be closed on
    // *every* return path, including a timeout or a write failure — otherwise
    // `medulla remote buildbox --exec 'sleep 1000'` times out after 30s here
    // and leaves the shell (and whatever it was running) alive in the daemon
    // forever. So the rest of the work happens in a helper whose `?`s all stay
    // inside it, and `close` runs unconditionally on the way out regardless of
    // what it returned.
    let outcome = run_in_watched_session(&mut host, &session_id, wanted.as_deref(), &command).await;
    close_and_confirm(&mut host, &session_id).await;
    outcome?;

    println!("{}", screen_text(&host));
    Ok(())
}

/// Watch `session_id`, run `command` in it (or just wait for a harness to
/// settle), and leave the screen ready for [`screen_text`].
///
/// Split out of [`run`] so every error path here — a timeout, a failed
/// `input`/`watch` — still lets the caller close the session; see the comment
/// at the call site.
async fn run_in_watched_session(
    host: &mut RemoteHost,
    session_id: &str,
    wanted: Option<&str>,
    command: &str,
) -> anyhow::Result<()> {
    host.watch(session_id)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    // A beat for the subscription to land before typing, so the first frame is
    // the shell's prompt rather than a blank screen.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A sentinel after the command, so we can tell "the output has arrived" from
    // "the screen has not caught up yet" without guessing at a delay.
    match wanted {
        // A shell: run the command and wait for a sentinel echoed after it, so
        // "the output has arrived" is distinguishable from "the screen has not
        // caught up" without guessing at a delay.
        None => {
            // Split across a quote so the *echoed command line* never contains
            // the literal the predicate looks for — only the shell's output
            // does. Counting two occurrences instead (command line + output)
            // fails whenever the command line scrolls off the 40-row screen,
            // which any long output does, and also whenever the command clears
            // the screen. One occurrence of a marker that can only come from
            // the output has neither problem.
            let marker = format!("__medulla_done_{}", std::process::id());
            let (head, tail) = marker.split_at("__medulla".len());
            host.input(format!("{command}; echo {head}''{tail}\n").as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            await_for("the command to finish", OUTPUT_TIMEOUT, host, |host| {
                // Twice: once in the echoed command line, once as the output.
                screen_text(host).contains(&marker)
            })
            .await?;
        }
        // A harness: there is no shell to echo a sentinel, so the command is
        // typed at it and the screen is read once it has settled. `--exec` is
        // the prompt here rather than a shell command.
        Some(_) => {
            await_for("the harness to start", OUTPUT_TIMEOUT, host, |host| {
                !screen_text(host).trim().is_empty()
            })
            .await?;
            if !command.trim().is_empty() {
                host.input(format!("{command}\n").as_bytes())
                    .await
                    .map_err(|e| anyhow::anyhow!(e))?;
                // A beat for the harness to answer. There is no sentinel to
                // wait on: what a harness prints is its own business.
                tokio::time::sleep(Duration::from_millis(1500)).await;
                host.pump().await;
            }
        }
    }
    Ok(())
}

/// Pump the host until `check` passes or the deadline expires.
async fn await_for(
    what: &str,
    timeout: Duration,
    host: &mut RemoteHost,
    mut check: impl FnMut(&RemoteHost) -> bool,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        host.pump().await;
        if check(host) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("timed out waiting for {what}")
}

/// The watched screen as text, trailing blanks trimmed.
fn screen_text(host: &RemoteHost) -> String {
    host.screen()
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .to_string()
        })
        .unwrap_or_default()
}

/// The configured host ids, for an error message worth reading.
fn known(sections: &[RemoteHostSection]) -> String {
    let names: Vec<String> = medulla::config::remote_hosts(sections)
        .into_iter()
        .map(|host| host.id)
        .collect();
    match names.is_empty() {
        true => "none are configured".to_string(),
        false => names.join(", "),
    }
}

/// Read `--name value` or `--name=value`.
fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == name {
            return it.next().cloned().filter(|value| !value.is_empty());
        }
        if let Some(rest) = arg.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return (!rest.is_empty()).then(|| rest.to_string());
        }
    }
    None
}

/// Close `session_id` and stay connected until the host stops listing it.
///
/// `LinkHandle::send` returns once a message is *queued*, not once it is
/// delivered — the link owns retransmission. Returning straight after the call
/// drops the link and stops it, so a single lost Close datagram would leave the
/// session and whatever it was running alive on a daemon that outlives this
/// process. That is the leak the caller is trying to prevent, reintroduced one
/// layer down.
///
/// So this waits for the host's own session list to stop naming it, which is the
/// only acknowledgement this protocol has. Bounded, and best-effort on timeout:
/// a host that has gone quiet is not a reason to hang a command that has already
/// produced its output.
async fn close_and_confirm(host: &mut RemoteHost, session_id: &str) {
    if host.close(session_id).await.is_err() {
        return;
    }
    let deadline = Instant::now() + CLOSE_TIMEOUT;
    while Instant::now() < deadline {
        host.pump().await;
        if !host.rows().iter().any(|row| row.id == session_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
