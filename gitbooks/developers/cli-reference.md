# CLI Reference

The `medulla` binary is the terminal app plus a small set of subcommands: the daemon that serves sessions on a remote machine, a one-shot remote command, sign-in, workflows, skills, and self-update.

| Command | What it does |
| --- | --- |
| `medulla` | Bare invocation starts the [TUI](#the-tui). |
| `medulla login` / `logout` | [OAuth login](authentication.md), via the browser or `--code` for SSH and other browserless terminals; logout clears the session. |
| `medulla remote <host> --exec <cmd>` | [Run one command](#medulla-remote) on a configured remote host and print what its screen showed. |
| `medulla daemon` | [Serve sessions](#medulla-daemon) on this machine to a Medulla elsewhere. `--direct` is what the SSH bootstrap starts on a remote host; a host key starts the one-client paired variant. |
| `medulla claude` / `codex` / `opencode` | [Wrappers](#harness-wrappers): run a CLI in this terminal, forwarding its transcript to a configured owner. |
| `medulla sessions` | List recent Claude Code and Codex sessions on this machine as JSON. |
| `medulla workflow <cmd>` | [Workflows](#medulla-workflow): author, inspect, and run saved multi-step plans. |
| `medulla skills <cmd>` | [Harness skills](#medulla-skills): write skills that trigger saved workflows into a harness's skill directory. |
| `medulla update` | [Self-update](#medulla-update): download, verify, and install the latest release. |
| `medulla version` / `help` | Version string; usage. |

Two more subcommands exist but are not for a person to type. `medulla mcp` serves Medulla's tools over MCP on stdin and stdout, and `medulla hook <Event>` is the shim Medulla installs as every launched harness's lifecycle hook. Medulla spawns both itself.

`medulla help` also lists `run`, `init`, `workspace`, and `hub`. Those belong to an earlier design in which a model above the sessions handed out work; they still execute but they are not part of the product described here.

## The TUI

A [ratatui](https://ratatui.rs/) app that holds a pseudo-terminal per session and draws the selected one. See [The TUI](the-tui.md) for the tabs and keys.

| Flag | Effect |
| --- | --- |
| `--config <path>` | Explicit config file (`.toml` or `.json`); bypasses layered discovery. |
| `--mock` | Run the scripted offline runtime, with no backend and no login screen. |
| `--no-alt-screen` | Stay on the main screen buffer, so the terminal's own scrollback keeps what Medulla drew. |

Unknown first arguments are treated as TUI arguments rather than rejected.

## `medulla remote`

Run one command on a `[[remoteHosts]]` entry and print the resulting screen:

```sh
medulla remote tower --exec "git status"
medulla remote tower --harness claude      # open a session there and print what it says
medulla remote tower --exec "make" --config ~/other/config.toml
```

The first non-flag argument is the host id from your config. `--exec` is required unless `--harness` names a harness to open, since a shell with nothing to run has nothing to report.

It goes through the same chain the TUI does: the SSH bootstrap, the pair key minted on the far side, the direct UDP link, a session open, keystrokes up, frames down, and the screen folded into text. It is the smallest thing that exercises every link, so the remote-host test suite is built on it, and it answers "what does the build box say" without opening the TUI. A stable `<host>-exec` identity is reused across invocations so each call does not strand another daemon on the far side.

## `medulla daemon`

A process that serves sessions on this machine to a Medulla somewhere else. There are two ways to run it.

### `--direct`: what the SSH bootstrap starts

```sh
medulla daemon --direct --peer-node <client node id> [--port <udp>] [--workspace <dir>] [--config <path>]
```

This is the command Medulla runs over `ssh` when you open a session on a remote host, and you rarely type it yourself. It mints a pair key for the one client named by `--peer-node`, binds a UDP port (`--port`, default ephemeral), prints one connect line to stdout, closes its stdio so the SSH channel can end, and keeps serving. There is no relay and no backend; the client is the only peer it will ever talk to.

`--workspace` is where sessions it serves start, and it is the most consequential flag: a harness serving a client edits files there. It defaults to the directory the daemon was launched in, so the shell that started it decides what the client can touch. The client passes its `[[remoteHosts]] workspace` here when one is set.

The daemon reads the far machine's own config (`~/.medulla/config.toml` there, or `--config`), never the client's. Its router, hooks, attribution, and custom presets are its own.

### The standing daemon

```sh
medulla daemon                # operator screen when stdout is a terminal
medulla daemon --headless     # service process; automatic when piped
```

A long-running daemon for a machine that serves work continuously. With a terminal attached it shows an operator screen (sessions and log, contacts, requests); `--headless` runs without one and is chosen automatically when the output is piped. On first run it names the worker and creates a host-link identity; `--reonboard` runs that again.

| Flag | Effect |
| --- | --- |
| `--tui` / `--headless` | Force the operator screen, or force a service process. |
| `--providers <a,b>` | Restrict to these coding agents (default: all found on `PATH`). |
| `--default-provider <name>` | The default harness among those available. |
| `--workspace <dir>` | Directory tasks run in (default: cwd). |
| `--name <label>` | Override the worker's advertised display name. |
| `--model <name>` | Default model hint passed to the harness. |
| `--opencode-agent <name>` | Agent name for the OpenCode provider. |
| `--concurrency <n>` | Maximum tasks running at once. |
| `--task-timeout-ms <n>` | How long a task may run. |
| `--once` | Drain the inbox once and exit (a probe). |
| `--no-onboard` / `--reonboard` | Skip, or redo, first-run registration. |
| `--no-pair` | Do not print the pairing block or copy the address. |
| `--dangerously-skip-permissions` | Headless path: pass the harness its bypass flag. |
| `--no-skip-permissions` | Operator screen path: keep the harness's permission prompts. |
| `--no-trust-workspace` | Do not pre-trust the workspace with Claude Code. |
| `--config <path>` | Explicit config file. |

The two permission flags point opposite ways on purpose. Headless, the bypass is opt-in and named for what it is. On the operator screen, sessions run unattended with the bypass on by default, because nobody is in the pane to answer a prompt and a task that stops on one hangs until it times out; `--no-skip-permissions` turns that off. The daemon's provider-spawn paths are unix-only.

### Paired daemon

`medulla daemon <host-key>` starts the one-client paired daemon described in the
[host-link protocol](host-link-protocol.md#72-host-key). The host key is a
one-shot bootstrap secret: its pair key is exposed in argv for that process
start, so use it only where local argv and shell-history readers are trusted.

## Harness wrappers

`medulla claude`, `medulla codex`, and `medulla opencode` launch the real CLI in your current terminal, exactly as if you had run it directly, with unrecognised flags passed through:

```sh
medulla codex resume            # args after the provider go to the CLI verbatim
medulla claude --model opus-4   # unrecognised flags pass straight through
medulla codex --no-bridge       # plain passthrough, nothing attached
medulla codex -- --no-bridge    # `--` forces everything after it to the child
```

Underneath, the wrapper tails the harness's own JSONL transcript and forwards each record as an encrypted event to the owner named by `MEDULLA_HARNESS_DM_TO` (or `MEDULLA_<PROVIDER>_DM_TO`); with inbound input enabled it also types the owner's replies into the child. With no owner configured it prints one warning and runs as a plain passthrough, which is also what `--no-bridge` asks for. OpenCode is always passthrough with input injection only, since its session log is not a flat JSONL. `MEDULLA_<PROVIDER>_BIN` overrides which binary is launched.

This is the bridge the standing daemon uses. A session opened from the TUI does not go through it; the TUI owns the PTY directly.

## `medulla sessions`

Lists the recent Claude Code and Codex sessions found on this machine as JSON, read from the harnesses' own transcript directories, so a script can pick one up without parsing them itself:

```sh
medulla sessions
```

## `medulla workflow`

Author, inspect, and run [workflows](../features/workflows.md): saved multi-step plans whose `agent` steps each run as a harness session.

```sh
medulla workflow list                  # every installed workflow
medulla workflow get <id>              # one workflow, whole
medulla workflow create <id>           # install from a document on stdin
medulla workflow delete <id>           # uninstall
medulla workflow apply-ops <id>        # apply graph patches from stdin
medulla workflow preview-ops <id>      # check those patches without saving
medulla workflow validate [id]         # validate a saved workflow, or stdin
medulla workflow catalog [kind]        # the node kinds an author may use
medulla workflow defaults <id>         # read or set a workflow's saved defaults
medulla workflow dry-run <id>          # simulate, dispatching nothing
medulla workflow run <id>              # run against the coding CLIs on this machine
medulla workflow resume <run-id>       # release approval gates and continue
medulla workflow cancel <run-id>       # stop a run executing in this process
medulla workflow list-runs <id>        # a workflow's run history
medulla workflow get-run <run-id>      # one run record
medulla workflow notes <id>            # notes filed against a workflow
medulla workflow note <id>             # file one
medulla workflow evolve <id>           # have a harness review the run history
medulla workflow proposals <id>        # proposals evolve filed
medulla workflow accept <id> <n>       # apply one; reject <id> <n> refuses it
medulla workflow mcp                   # serve the workflow tools over MCP
```

| Flag | Effect |
| --- | --- |
| `--input <json>` | Trigger payload for `run` / `dry-run`. |
| `--inputs <json>` | Declared workflow inputs, as an object keyed by name. |
| `--set <name>=<value>` | One declared input (repeatable; wins over `--inputs`). |
| `--workspace <dir>` | Checkout the run works in: where shell steps run and which repository agent steps open (default: cwd). |
| `--run-id <id>` | Id to give the run (default: a fresh one). |
| `--approve <node-id>` | Gate to release on `resume` (repeatable). |
| `--reject <node-id>` | Gate to refuse on `resume` (repeatable). |
| `--config <path>` | Explicit config file (`.toml` or `.json`). |

Workflow documents and graph ops are read from stdin, and every verb prints JSON.

`cancel` is process-local: a run started by `medulla workflow run` in one shell cannot be cancelled from another, because there is no control channel between two CLI invocations, and the command says so. The TUI can always cancel the run it started.

`medulla workflow mcp` is not for a person to run. It is the command Medulla attaches to a harness session so the agent on the other end can author and run workflows itself. `medulla mcp` is the same server with every tool family Medulla exposes, scoped by the grant the caller was handed.

## `medulla skills`

Write harness-native skills that trigger saved [workflows](../features/workflows.md) to disk, so an everyday Claude Code or Codex session run outside Medulla can still find and start one:

```sh
medulla skills list                    # the managed skills currently on disk
medulla skills install                 # write skills for every enabled workflow
medulla skills sync                    # reconcile disk with the current workflow set
medulla skills sync --prune            # also remove skills for workflows that are gone
medulla skills uninstall <id...>       # remove skills for named workflows
medulla skills uninstall --all         # remove every managed skill
```

With no verb it lists. `install` (alias `add`) and `sync` (alias `refresh`) take zero or more workflow ids, all enabled workflows when none are named; `uninstall` (aliases `remove`, `rm`) needs either explicit ids or `--all`.

| Flag | Effect |
| --- | --- |
| `--harness <a,b>` | `claude`, `codex`, `generic`, or `all` (default: those already set up). |
| `--scope <user\|project\|managed>` | Install into `$HOME`, into this checkout, or into Medulla's own root that spawned harnesses are pointed at (default: `user`). |
| `--dir <path>` | Explicit root, overriding `--scope`. |
| `--with-mcp` | Also register `medulla mcp` with each harness. |
| `--with-commands` | Also write the slash-command variant. |
| `--tools <run\|full>` | Tool surface a skill-triggered session gets (default: `run`). |
| `--prune` | Remove skills for workflows that no longer exist (`sync`, with no ids). |
| `--all` | Remove every managed skill (`uninstall`, instead of naming ids). |
| `--dry-run` | Report what would change and write nothing. |
| `--json` | Emit JSON instead of the human summary. |

Unlike the rest of the CLI, an unrecognised flag here is a hard error. Every flag decides where files get written, so a dropped `--dir` would retarget the whole operation at `$HOME` while reporting success. `--prune` with explicit ids is refused for the same reason.

## `medulla login` and `logout`

```sh
medulla login                    # browser loopback flow
medulla login --code             # paste a code: works over SSH
medulla login --no-browser       # print the URL instead of opening one
medulla login --token <64-hex>   # redeem a one-time token, headless
medulla logout
```

| Flag | Effect |
| --- | --- |
| `--provider <name>` | OAuth provider: `google` (default), `github`, `twitter`. |
| `--no-browser` | Print the login URL without launching a browser. |
| `--code` | Sign in by pasting a code; open the URL on any device. |
| `--token <64-hex>` | Redeem a one-time login token instead. |
| `--config <path>` | Config file to read `backend.tokenEnv` from. |

[Authentication](authentication.md) covers the flows and where credentials are stored.

## `medulla update`

```sh
medulla update           # download, verify (sha256), and install the latest release
medulla update --check   # only report whether a newer version is available
```

`update` downloads the platform asset named in the release's `latest.json` manifest, verifies its SHA-256, extracts the binary, and atomically replaces the running executable, keeping the previous one as `<exe>.old`. It refuses when the executable path is not writable.

The TUI also checks about ten seconds after startup and every six hours, and shows an "update available" banner in the header. `[update] check = false` or `MEDULLA_NO_UPDATE_CHECK=1` turns that off; `MEDULLA_UPDATE_URL` points it at a different manifest.
