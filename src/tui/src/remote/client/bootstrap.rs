//! Starting a daemon on another machine and learning how to reach it.
//!
//! # Why this shells out to `ssh`
//!
//! There is no SSH crate in this workspace, and adding one would mean owning
//! host-key verification, agent forwarding, `~/.ssh/config`, `ProxyJump`,
//! `Match` blocks, hardware keys and GSSAPI — all of which the operator's own
//! `ssh` already does, already has their configuration for, and already has
//! their `known_hosts` file. A library implements the protocol, not any of that.
//! Reimplementing host-key trust-on-first-use inside a modal is a security
//! surface we would own forever.
//!
//! Mosh made the same call for the same reasons, and this is mosh's bootstrap.

use std::process::Stdio;

use medulla::config::RemoteHostSection;
use medulla_link::keys::NodeId;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::remote::serve::ConnectLine;

/// How long to wait for the far side to print its connect line.
///
/// Generous, because everything before that line is out of our hands: a slow
/// login, a MOTD, a `ProxyJump` through a bastion.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a bootstrap did not produce a usable host.
#[derive(Debug)]
pub enum BootstrapError {
    /// `ssh` itself failed — a refused connection, a rejected key, an unknown
    /// host key. Carries its stderr, which is almost always the real
    /// explanation and is far more useful than anything we could paraphrase.
    Ssh {
        /// `ssh`'s exit status.
        status: Option<i32>,
        /// Whatever it wrote to stderr.
        stderr: String,
    },
    /// The remote command was not found. Distinguished from a general failure
    /// because it is the most likely first-run problem and has a specific fix.
    NotInstalled {
        /// The host that was reached.
        host: String,
        /// The command that was looked for there.
        command: String,
    },
    /// The far side ran but never printed a connect line.
    NoHandshake {
        /// The host that was reached.
        host: String,
        /// The command that ran there.
        command: String,
        /// What it did print, for diagnosis.
        output: String,
    },
    /// The local `ssh` binary could not be started at all.
    Spawn(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Ssh { status, stderr } => {
                let stderr = stderr.trim();
                match stderr.is_empty() {
                    // `255` is ssh's own "I failed"; anything else came from the
                    // remote command.
                    true => write!(f, "ssh failed with status {status:?}"),
                    false => write!(f, "{stderr}"),
                }
            }
            BootstrapError::NotInstalled { host, command } => write!(
                f,
                "{host} has no `{command}` on its PATH — install medulla there \
                 (curl -fsSL https://medulla.tinyhumans.ai/install.sh | sh), or set \
                 `remoteCommand` for this host to where it lives"
            ),
            BootstrapError::NoHandshake {
                host,
                command,
                output,
            } => write!(
                f,
                "`{command}` on {host} started but printed no connect line — is it \
                 a different program? It said: {}",
                output.trim()
            ),
            BootstrapError::Spawn(error) => write!(f, "could not run ssh: {error}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

/// Build the `ssh` argv for one host.
///
/// Everything here is public. The client's node id travels in argv because a
/// node id rides in cleartext outer headers by design; the *key* never does, and
/// that is the whole reason the far side mints it and prints it back. A key
/// passed as an argument would sit in the remote machine's `ps` output, readable
/// by every user on it, for as long as the daemon ran.
pub fn ssh_argv(host: &RemoteHostSection, client: NodeId, workspace: &str) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    if host.port != 0 {
        argv.push("-p".to_string());
        argv.push(host.port.to_string());
    }
    if !host.identity_file.trim().is_empty() {
        argv.push("-i".to_string());
        argv.push(host.identity_file.trim().to_string());
    }
    argv.extend(host.ssh_options.iter().cloned());
    argv.push(match host.user.trim() {
        "" => host.host.trim().to_string(),
        user => format!("{user}@{}", host.host.trim()),
    });
    argv.push("--".to_string());
    // `ssh host -- a b c` does not exec `a` with argv `[b, c]`: everything after
    // `--` is joined with spaces into one string and handed to the remote login
    // shell, which re-tokenizes it. So each word of the command tail must be
    // shell-quoted individually here, before ssh ever concatenates them —
    // otherwise a workspace path with a space becomes two arguments, and one
    // with `$()` or `;` gets evaluated by the remote shell.
    argv.push(posix_quote(&remote_command(host)));
    argv.push("daemon".to_string());
    argv.push("--direct".to_string());
    argv.push("--peer-node".to_string());
    argv.push(client.to_string());
    if !workspace.trim().is_empty() {
        argv.push("--workspace".to_string());
        argv.push(posix_quote(workspace.trim()));
    }
    // Backgrounded, deliberately unquoted so the remote shell reads it as an
    // operator rather than an argument. This is what lets `ssh` return.
    //
    // `sshd` waits for the command *process* to exit, so a daemon that merely
    // closed its stdio would still hold the session open forever — the client
    // would leave one `ssh` behind per bootstrap, and the daemon would die
    // whenever that connection eventually dropped, taking every "persistent"
    // session with it. Backgrounding makes the shell exit immediately; the
    // daemon then closes stdio itself (`serve::detach_from_ssh`), which is what
    // lets the channel close and `ssh` finish.
    //
    // The connect line still arrives: `ssh` drains the pipe until every writer
    // closes it, and the daemon prints and flushes *before* detaching.
    //
    // `&` rather than `setsid --fork`, which is util-linux and absent on macOS —
    // a remote host this must work on.
    argv.push("&".to_string());
    argv
}

/// How `medulla` is invoked on the far side.
pub fn remote_command(host: &RemoteHostSection) -> String {
    match host.remote_command.trim() {
        "" => "medulla".to_string(),
        value => value.to_string(),
    }
}

/// Quote one word for the POSIX shell that receives ssh's command tail.
///
/// Plain single-quoting, with embedded quotes escaped by closing the quoted
/// string, appending an escaped quote, and reopening it — the standard
/// `'\''` trick. Every character is safe inside single quotes except `'`
/// itself, so this needs no allowlist of "safe" characters to get wrong.
fn posix_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The SSH client to run.
fn ssh_binary(host: &RemoteHostSection) -> String {
    match host.ssh_binary.trim() {
        "" => "ssh".to_string(),
        value => value.to_string(),
    }
}

/// A daemon started on `host`, and the child holding its SSH session open.
pub struct Bootstrapped {
    /// How to reach it over UDP.
    pub connect: ConnectLine,
    /// `host:port` for the direct link.
    pub endpoint: String,
    /// The `ssh` process, for the caller to reap.
    ///
    /// It exits on its own now: the daemon closes its stdio when it detaches, so
    /// `ssh` sees EOF and returns. Held rather than detached so the caller can
    /// wait for it and not leave one behind per bootstrap.
    pub child: tokio::process::Child,
}

/// Start a daemon on `host` and read back how to reach it.
///
/// `client` is this endpoint's node id, which the far side enrolls a fresh pair
/// key against.
///
/// # Errors
///
/// See [`BootstrapError`]. Each case is distinguished because each has a
/// different fix, and a generic "connection failed" here costs the operator an
/// hour.
pub async fn bootstrap(
    host: &RemoteHostSection,
    client: NodeId,
    workspace: &str,
) -> Result<Bootstrapped, BootstrapError> {
    let argv = ssh_argv(host, client, workspace);
    let mut child = Command::new(ssh_binary(host))
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false)
        .spawn()
        .map_err(|error| BootstrapError::Spawn(error.to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BootstrapError::Spawn("ssh produced no stdout".to_string()))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut seen = String::new();

    // Scan rather than read one line: a login shell prints a MOTD, a
    // `Last login:` banner and whatever else it likes before our command ever
    // runs, and none of that is an error.
    let found = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(connect) = ConnectLine::parse(&line) {
                return Some(connect);
            }
            seen.push_str(&line);
            seen.push('\n');
        }
        None
    })
    .await
    .unwrap_or(None);

    match found {
        Some(connect) => {
            let udp_host = match host.udp_host.trim() {
                "" => host.host.trim(),
                value => value,
            };
            Ok(Bootstrapped {
                // Bracketed when it is a bare IPv6 literal: `format!("{h}:{p}")`
                // turns `::1` into `::1:9`, which parses as a different address.
                endpoint: crate::remote::serve::join_endpoint(udp_host, connect.port),
                connect,
                child,
            })
        }
        None => Err(diagnose(host, child, seen).await),
    }
}

/// Work out why no connect line arrived, and say the most useful thing.
async fn diagnose(
    host: &RemoteHostSection,
    mut child: tokio::process::Child,
    output: String,
) -> BootstrapError {
    // On the EOF path stdout closing already means ssh has exited, so `wait`
    // returns promptly. On the timeout path it did not: `kill_on_drop(false)`
    // means nothing has asked ssh (or whatever it started but never printed a
    // connect line for) to stop, and `wait` below would hang until the remote
    // process happens to exit on its own — which, for a daemon, may be never.
    // `start_kill` is harmless if the process already exited.
    let _ = child.start_kill();
    let status = child.wait().await.ok().and_then(|status| status.code());
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use tokio::io::AsyncReadExt;
        let _ = pipe.read_to_string(&mut stderr).await;
    }
    let command = remote_command(host);
    // 127 is the shell's "command not found"; ssh reports the remote command's
    // status as its own. 255 is ssh failing before the command ever ran.
    let missing = status == Some(127)
        || stderr.contains("command not found")
        || stderr.contains("No such file or directory");
    if missing && status != Some(255) {
        return BootstrapError::NotInstalled {
            host: host.host.trim().to_string(),
            command,
        };
    }
    if status == Some(255) || !stderr.trim().is_empty() {
        return BootstrapError::Ssh { status, stderr };
    }
    BootstrapError::NoHandshake {
        host: host.host.trim().to_string(),
        command,
        output,
    }
}
