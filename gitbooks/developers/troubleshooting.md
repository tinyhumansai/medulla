---
description: >-
  Diagnosing the failures Medulla actually produces: install, login, remote hosts, clipboard through tmux and SSH, and where the logs are.
---

# Troubleshooting

## Install and startup

### `medulla` is not on `PATH` after installing

The installers write to `~/.medulla/bin` (`%USERPROFILE%\.medulla\bin` on
Windows) and append it to your shell profile or user `PATH`. That change reaches
only new shells, so open a new terminal or run `exec $SHELL`. Invoking the binary
directly from the install prefix always works. If you set
`MEDULLA_NO_MODIFY_PATH=1`, nothing was appended and you have to add the
directory yourself.

### The binary dies on a missing symbol version (Linux)

The prebuilt Linux binaries are built on Ubuntu 24.04 and need glibc 2.39 or
newer. That covers Ubuntu 24.04+, Debian 13+, and current Fedora, but not RHEL 9
and its rebuilds (Rocky, AlmaLinux), Debian 12, or Amazon Linux 2023, which ship
glibc 2.34 to 2.36.

`install.sh` runs the binary once immediately after installing it for exactly
this reason. If it cannot start, the installer removes it, says why, and falls
back to building from source, which works anywhere Rust does. Install Rust from
[rustup.rs](https://rustup.rs/) and re-run. With no `cargo` present the installer
stops and explains rather than leaving a file that only fails when you try to use
it. See [Getting Started](getting-started.md#the-linux-glibc-floor).

### `medulla-tui requires an interactive terminal (TTY).`

The TUI exits 1 when stdout is not a terminal. Run it in a real terminal, or use
a non-interactive subcommand: `medulla run` for one instruction, `medulla daemon`
(which selects headless automatically when stdout is not a terminal) for a
worker.

## Login

### The browser flow never completes over SSH

It cannot. The backend redirects your browser to `http://127.0.0.1:<port>`, which
is the loopback interface of the machine running the browser, not the remote host
running Medulla, so the listener there never sees the callback.

Use the code flow instead:

```sh
medulla login --code
```

It binds nothing, prints `<baseUrl>/auth/<provider>/login?redirect=cli`, and
waits on stdin. Open that URL on any device, sign in, and paste the one-time code
the page shows back into the terminal.

### The code is rejected

The code is single-use and expires after 15 minutes, and
`/auth/login-token/consume` deletes it atomically. A code that has already been
redeemed, including by a retry you forgot about, is inert. A rejected value
leaves you on the same screen with the verification URL still up, so fetch a
fresh code from it.

A submitted value is classified by shape: 64 lowercase hex is redeemed as a login
code, anything else is treated as a ready-made JWT. A JWT pasted into the code
field therefore takes the wrong path.

### The loopback callback returns 400

The listener appends a random 32-hex state nonce to the `redirectUri` and rejects
any `/auth` callback whose `state` is missing or mismatched, while continuing to
wait. That normally means a stale browser tab from an earlier attempt landed on
the listener. Start the flow again and use the fresh URL. The listener also
replies 405 to non-GET, 404 to anything that is not `/auth`, drops non-loopback
peers, and bounds each connection with a 5 s read timeout and an 8 KiB buffer.

### The TUI opens a login screen even though `medulla login` said it worked

Something outranks the stored session, or the session was stored somewhere this
process does not read. The credential chain is inline `backend.token`, then
`backend.tokenEnv` (default `MEDULLA_TOKEN`), then `<home>/session.json`, and
`medulla login` now refuses to store a session underneath one of the first two
rather than saving one nothing would read, so a login that reports this is
telling you which source to remove.

A stored session is also scoped to the deployment that issued it: it records its
own `baseUrl` and is only offered to a backend with a matching origin. A session
minted against another deployment — by an older build, back when the endpoint was
configurable — will not be offered to the pinned one; sign in again.

Older installs kept a separate `credentials.json`, which could report success
while the runtime stayed signed out. `login` now adopts that file, verifying its
JWT and rewriting it as a proper session, and only `logout` deletes it. See
[Authentication](authentication.md#upgrading-from-a-standalone-credentials-file).

Check which account you are on. The active account is recorded in
`<root>/active_user.toml`, and `MEDULLA_USER=<id>` selects a different one for a
single process ahead of that marker. `MEDULLA_USER=local` reaches the pre-login
home. See [Medulla home](configuration.md#medulla-home).

### No backend configured: it stops instead of offering a login screen

Readiness is three states, not two, because a host answers each differently: run,
sign in, or stop. Reachable but signed out opens the login flow. No backend URL
at all reports that error instead, because a login screen cannot fix a missing
base URL. The endpoint is pinned into the binary now, so this is no longer
something a config or environment variable can get wrong. A build that reports it
is a build whose constant is empty, which is a bug worth filing.

To get a working interface with no backend at all, ask for the mock runtime:
`medulla --mock`. The login screen deliberately does not offer it: a failed
sign-in should not quietly land you in a scripted demo you might mistake for the
product.

## Remote hosts

### The host row says failed

The failure carries the reason, and the three worth knowing are different
problems with different fixes.

An unknown or changed host key, a refused login, or an unreachable machine is
`ssh` failing, and the message is `ssh`'s own stderr. Fix it the way you would
fix `ssh <host>` at a shell prompt, because that is literally what ran: your
`~/.ssh/config`, your keys, and your `known_hosts` are what Medulla used.

`<host> has no medulla on its PATH` means the login worked and the binary is
missing on the far side. Install it there with the same installer, or set
`remoteCommand` to its full path if it is installed somewhere the login shell
does not see.

A bootstrap that times out after thirty seconds usually means something before
the connect line took too long: a slow MOTD, a `ProxyJump` through a bastion, or
a login shell that blocks. Run the ssh command by hand and see what it prints
before the `MEDULLA-CONNECT v1` line.

### It connected, but nothing happens after picking the host

The picker waits on the host step until the far side answers, because the
harness list has to come from that machine. If the daemon started but its UDP
port cannot be reached, for instance because SSH went through a jump host and
UDP cannot follow, set `udpHost` on the entry to the address UDP should use.

### The remote list offers no harness

The far side detects the coding CLIs on the `PATH` of the shell that started the
daemon, which is a non-interactive login shell over SSH. A CLI installed by a
tool that only edits `.bashrc` or `.zshrc` will not be on that `PATH`. Put it
somewhere the login shell sees, or export it from `.profile` or `.zshenv`.

### Sessions there run in the wrong directory

The daemon's `--workspace` is the `workspace` on your `[[remoteHosts]]` entry,
and with none set it is the directory the daemon was launched in, which over SSH
is the login home. Set `workspace`, and list any others under `workspaces` so the
picker can offer them; a remote directory cannot be completed from here.

### The remote row's "waiting" timer looks wrong

It is measured from when your machine first saw the cue, not from the remote
clock, so it cannot be skewed by the host's time. A timer that resets is a cue
that changed to a different one, which is what "how long has it been asking this"
means.

### The standing daemon: two peers never hear from each other

For an enrolled `medulla daemon` (not the SSH-bootstrapped `--direct` case), the
usual cause is that the two ends are pointed at different forwarders. The daemon
states its identity and forwarder together on its first log line
(`host link: <id> on <endpoint>`), and the client prints its own. Compare them.

A peer being `Offline` is not terminal. The link keeps retransmitting through
`Degraded` and `Offline`, and recovery needs no reconnect or re-enrollment. See
[liveness](host-link-protocol.md#62-liveness). There is no key recovery: the
backend never holds a pair key, so a lost key means re-enrolling the host.

## Copying out of Medulla

Medulla is usually not running where you are sitting. A typical session is a tmux
on your laptop, an SSH connection, a second tmux on the box, and the TUI inside
that, with a coding-agent harness on a pty inside the TUI. Every copy has to
cross those layers to reach the clipboard you actually paste from.

### What can copy

A drag over the TUI copies the swept block, read back out of the rendered frame,
so this works over any pane, including a harness pane.

A short line, such as a worker address or a command, can be copied on its own.

The harness itself can copy: anything the wrapped agent copies (OSC 52),
including a `tmux load-buffer -w` run inside its own pane. Medulla is that
harness's terminal, so its copy arrives here and is forwarded on rather than
dropped.

### The routes a copy takes

Nothing in this chain can be confirmed from Medulla's side, so it does not pick
one route. It takes every route that could work, all carrying the same text:

1. The OSC 52 escape, written to Medulla's own stdout. Handled by the terminal at
   the far end of SSH, if every multiplexer in between forwards it.
2. The same escape wrapped in tmux's DCS passthrough, which a tmux with
   `allow-passthrough on` hands upwards verbatim. That is one hop out of the
   inner tmux, where the outer tmux then sees an ordinary OSC 52 from a pane.
3. `tmux load-buffer -w` against the socket in `$TMUX`, which needs no terminal
   capability and no configuration at all. It reaches an accessible tmux server
   without either of those, but it still needs that server to be reachable, so it
   can fail (a dead socket, tmux missing). When it succeeds, worst case you paste
   from the tmux buffer with `prefix ]`.
4. A local clipboard binary (`pbcopy`, `wl-copy`, `xclip`, `xsel`), so a Medulla
   running on the machine you are sitting at lands in the real clipboard.

The status line names what actually took it, for example
`Copied selection → clipboard (tmux buffer + OSC 52)`. A status of
`Sent … → terminal` means only routes 1 and 2 went out and nothing acknowledged
them.

### Making the nested case reach your laptop

Routes 3 and 4 need no setup. For the escape to travel the whole way to the
terminal on your laptop, each tmux in the chain has to agree to pass it on:

```tmux
# On the box (the inner tmux): forward what medulla wraps for us, and take
# clipboard writes into our own buffer as well.
set -g allow-passthrough on
set -g set-clipboard on

# On the laptop (the outer tmux): accept OSC 52 from a pane and hand it to the
# terminal emulator.
set -g set-clipboard on
```

Your terminal emulator must also allow OSC 52 writes. iTerm2 and WezTerm do by
default, and some terminals gate it behind a setting.

`allow-passthrough` defaults to off in tmux 3.3 and later, which is why the inner
tmux is the layer that usually needs the change.

### Where this lives in the code

| Path | Contents |
| --- | --- |
| [`src/sdk/src/clipboard/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/clipboard/) | The routes themselves. `tmux.rs` holds the passthrough wrapper, the `load-buffer` hop, and the OSC 52 parameter parsing. |
| `src/tui/src/worker/pty/manager/clipboard.rs` | Forwarding a harness's own copy out of its pane. |
| `src/tui/src/ui/app/render/selection.rs` | The drag-to-select path. |

## Logs

Medulla narrates itself through one line sink per process. Each sink keeps the
last 2000 lines in memory for the screen and mirrors every line, timestamped, to
a file.

| Process | File |
| --- | --- |
| The TUI | `<medulla home>/logs/orchestrator.log` (the name is historical) |
| The daemon | `<medulla home>/logs/worker.log` |

`MEDULLA_LOG_DIR` overrides the directory. The default is deliberately not the
workspace: a worker's workspace is full of real repositories, and a log file
dropped into one invites it into a commit.

A log is rotated once it passes 8 MiB, moved aside as `<name>.log.1`, so a
long-lived daemon cannot fill a disk unattended. One generation back is kept.
Lines are flushed per line rather than buffered, so a crash does not lose exactly
the lines that explain it.

File logging is best effort throughout. An unwritable directory disables the file
and leaves the in-memory ring working rather than failing the start, so a missing
log file is not itself an error. When the file did open, the TUI reports its path
at startup (`logging to <path>`) as the last item in its startup-status chain.

There is no `RUST_LOG`-style env filter and no `tracing-subscriber` in the
binary: what a process writes is what it narrates through its sink, not a
level-filtered trace. `medulla daemon --headless` sends the same lines to stderr,
so redirecting stderr captures a daemon's narration without going through the
log file at all.

For a failure inside a test harness rather than a live run, see
[Testing](testing.md).

## Read next

* [Configuration](configuration.md): home directory, layered config, remote hosts.
* [Authentication](authentication.md): the full login flows and security model.
* [Environment variables](environment-variables.md): every variable named on this page.
