# CLI Reference

The `medulla` binary is both the terminal app and a small suite of subcommands
for headless operation, bridging coding-agent harnesses to
the host link, and self-updating.

| Command | What it does |
| --- | --- |
| `medulla` | Bare invocation starts the [TUI](#the-tui). |
| `medulla run <instruction>` | [Headless one-shot](#medulla-run): submit one instruction and stream the cycle's events as JSON lines. |
| `medulla login` / `logout` | [OAuth login](authentication.md), via the browser or `--code` for SSH and other browserless terminals; logout clears the session and keeps the account selected. |
| `medulla daemon` | [Coding-agent worker daemon](#medulla-daemon) over the host link (`--headless` for a service process). |
| `medulla codex` / `claude` / `opencode` | [Harness wrappers](#harness-wrappers): run a CLI, bridged to your orchestrator. |
| `medulla sessions` | List recent claude/codex sessions as JSON. |
| `medulla workflow <cmd>` | [Workflows](#medulla-workflow): author, inspect, and run multi-step plans. |
| `medulla init [dir]` | [Draft a MEDULLA.md](#medulla-init) workspace profile. |
| `medulla workspace <cmd>` | [Workspace registry](#medulla-workspace): `add [dir]` / `list` / `remove <dir\|id>`. |
| `medulla hub` | [Relay hosted-backend tasks](#medulla-hub) to configured host-link workers. |
| `medulla update` | [Self-update](#medulla-update): download, verify, install the latest release. |
| `medulla version` / `help` | Version string; usage. |

Unknown first arguments are treated as arguments to the main TUI rather than
rejected as unknown subcommands.

## The TUI

A [ratatui](https://ratatui.rs/) terminal UI over the SDK: chat with the
orchestrator and watch agent lanes, traces, and context live. It drives the
Medulla backend directly over the network — there is no embedded core process
to boot and no socket to attach to. See [Runtimes](configuration.md#runtimes)
for what happens when nobody is signed in, and
[Upgrading from the external core socket](configuration.md#upgrading-from-the-external-core-socket)
if you are coming from a version that used `--core-socket`.

TUI flags:

| Flag | Effect |
| --- | --- |
| `--config <path>` | Explicit config file (`.toml` or `.json`); bypasses layered discovery. |
| `--mock` | Force the scripted offline runtime and skip the token lookup and login screen. |
| `--no-alt-screen` | Stay on the main screen buffer (useful for scrollback while debugging). |

The tabs are Overview, Sessions, Workflows, Subconscious, Changes, Hosts,
Feedback, and Settings. Workflows is present only in a build with the default
`workflows` feature. See [The TUI](the-tui.md#the-tabs) for what each one holds and for the
surfaces that are not in the tab bar of this build.

## `medulla run`

A headless, scriptable path to a single instruction, and the one to drive from CI
or a container, since it needs no TTY. It boots the same embedded core the TUI
uses, submits one instruction, and streams the folded cycle events to stdout as
newline-delimited JSON:

```sh
medulla run "audit the payments service for unbounded retries"
medulla run --config ./medulla.toml "..."
```

Everything that is not a flag is joined into one instruction; with no instruction
it errors. It emits each folded event as a JSON line and returns when the cycle
ends. It binds the core's state directory, action directory, and endpoints from
the resolved config before booting, so a scripted run pointed at `MEDULLA_HOME`
reads and writes that home rather than the developer's real one.

| Flag | Effect |
| --- | --- |
| `--config <path>` | Explicit config file (`.toml` or `.json`). |

## `medulla daemon`

A headless coding-agent daemon that serves
[claude](https://www.anthropic.com/claude-code),
[codex](https://github.com/openai/codex), and
[opencode](https://github.com/sst/opencode) over encrypted host-link datagrams. On
first launch it runs a one-time [worker registration](#first-run-worker-registration)
flow. `medulla daemon --reonboard` forces that flow again.

When stdout is a terminal it selects the operator UI; `--headless` forces a
service process, and a non-terminal launch selects headless automatically. The
operator UI asks two things before it listens for work: an execution
mode (Interactive holds live, watchable harness sessions; Headless launches one
process per task and shows logs) and a default harness from the providers
detected on `PATH`. It then presents three tabs: Agents (live sessions with
an embedded PTY you can attach to or take over), Master (the worker address,
saved masters, and operator messages), and Workspaces (the canonicalized
roots the worker may advertise). Enrollment is the entire admission decision;
there is no separate approval queue.

The daemon creates and stores a worker-level link identity locally; it does
not need the master's backend token. Mode, harness, workspace, and master
choices persist to the Medulla config, so the usual setup needs no environment
variables.

Daemon flags:

| Flag | Effect |
| --- | --- |
| `--headless` | Run without the operator screen (automatic when piped). |
| `--tui` | Force the operator screen. |
| `--providers <a,b>` | Restrict the accepted harnesses (default: all found on `PATH`). |
| `--default-provider <name>` | Choose the default harness among those available. |
| `--workspace <dir>` | Set the primary task working directory (default: cwd). |
| `--handle <name>` | Register an `@handle` on startup. |
| `--name <label>` | Override the worker's advertised display name. |
| `--model <name>` | Supply a default model hint passed to the harness. |
| `--opencode-agent <name>` | Agent name for the OpenCode provider. |
| `--skills <a,b>` | Extra skills to advertise. |
| `--concurrency <n>` | Cap simultaneous executions. |
| `--once` | Drain the current inbox once and exit (a probe). |
| `--no-onboard` | Skip key publishing and directory registration. |
| `--reonboard` | Replace the stored worker registration. |
| `--no-pair` | Do not print the pairing block or copy the address. |
| `--dangerously-skip-permissions` | Headless path: pass the provider's unsafe permission switch. |
| `--no-skip-permissions` | Operator-screen path: *keep* the provider's permission prompts. |
| `--no-trust-workspace` | Do not pre-trust a Claude workspace in the operator-screen path. |
| `--config <path>` | Explicit config file. |

The primary `--workspace` is the execution directory; any additional advertised
roots are capability metadata, not per-task working directories.

The two permission flags point opposite ways, which is deliberate. In the
headless path the bypass is opt-in and visibly named: `--dangerously-skip-permissions`
hands the harness the provider's unsafe switch. On the operator screen peer
sessions run unattended and with the bypass on by default, because nobody is
in the pane to answer a prompt and a task that stops on one has hung until it
times out; `--no-skip-permissions` turns that off. For the same reason the
operator path clears Claude's fresh-directory trust dialog up front (naming the
workspace at launch is taken as the decision to run peer work there), and
`--no-trust-workspace` declines on your behalf instead.

### Pairing a worker to an orchestrator

Pairing needs one string to travel, the worker's address. On startup the daemon
prints it and also copies it to **your** terminal's clipboard rather than the
remote machine's, using OSC 52, so it survives an SSH boundary. In the
orchestrator, Hosts › Add Host takes it with `a`; `c` there copies a
one-line installer to paste into an SSH session on the machine you are adding.

OSC 52 needs a terminal that accepts it: tmux wants `set -g set-clipboard on`,
and some terminals disable it for security. The copy is also skipped when the
daemon's output is piped. The address is printed on a line of its own either way.
`--handle build-box` skips the copy entirely: type `@build-box` into Add Host.
Pass `--no-pair` when the output is being parsed by a script.

## Harness wrappers

`medulla codex` / `medulla claude` / `medulla opencode` launch the real
coding-agent CLI in your terminal exactly as if you had run it directly, with
unrecognized flags passed through verbatim, while bridging the
session to the host link underneath. The wrapper tails the harness's own JSONL
transcript, normalizes each record into a typed `SessionEnvelopeV2` event, and
forwards the stream as encrypted [Signal-protocol](https://signal.org/docs/) DMs
to the configured owner; with inbound input enabled it also polls for
owner→session control frames and types their text into the child.

With inbound input disabled the child simply inherits your stdio. With it
enabled the harness is run on a pseudo-terminal instead, so a full-screen TUI
still sees a real terminal on stdin and keeps its own echo, Ctrl-C handling, and
resize behaviour while owner messages are typed in alongside your keystrokes.

```sh
medulla codex resume            # any args after the provider go to the CLI verbatim
medulla claude --model opus-4   # unrecognized flags pass straight through
medulla codex --no-bridge       # pure passthrough: run the CLI with no host-link bridge
medulla codex -- --no-bridge    # `--` forces everything after it to the child
```

Configuration is by environment variable. The `TINYPLACE_*` spelling of each of
these is deprecated but still read, directly behind the `MEDULLA_*` name, so a
host configured before the rename keeps working:

| Variable | Effect |
| --- | --- |
| `MEDULLA_HARNESS_DM_TO` / `MEDULLA_<P>_DM_TO` / `MEDULLA_OPENHUMAN_OWNER` | Owner to forward the session envelopes to. |
| `MEDULLA_HARNESS_RECEIVE_FROM` / `MEDULLA_<P>_RECEIVE_FROM` | Peer whose input control frames / plain DMs are injected (defaults to the owner). |
| `MEDULLA_HARNESS_RECEIVE=0` / `MEDULLA_<P>_RECEIVE=0` | Disable inbound input injection. |
| `MEDULLA_<P>_BIN` (`MEDULLA_CODEX_BIN`, `MEDULLA_CLAUDE_BIN`, `MEDULLA_OPENCODE_BIN`, `MEDULLA_OPENHUMAN_BIN`) | Override the provider binary. OpenHuman's bare `OPENHUMAN_BIN` predates the convention and is still read behind the namespaced pair. |
| `MEDULLA_HARNESS_MODEL` / `MEDULLA_<P>_MODEL` | Override the model a turn runs on. Read today by the embedded OpenHuman harness (`MEDULLA_OPENHUMAN_MODEL`); see [Custom harness presets](configuration.md#custom-harness-presets) for the full precedence. |
| `MEDULLA_<P>_SESSIONS_DIR` | Override the transcript directory the tailer watches. |

If no owner is configured (and `--no-bridge` was not passed), the wrapper prints
a single warning and runs as a plain passthrough.

### Scope notes

This is the single-terminal `--raw` wrapper. It does not build
the upstream TUI chrome, the `--agent` plugin mode, the machine-bus
multi-terminal coordination, the opencode SSE server, or the terminal-envelope
writer. `medulla
opencode` runs as a passthrough with input injection but no transcript tailing
(its session log is not a flat JSONL the mappers read).

## First-run worker registration

The first time a worker starts, whether `medulla daemon` or a bridged `medulla
codex|claude|opencode`, it runs a one-time onboarding flow that names the worker
and connects it to an owner, then persists a small profile at
`<medulla-home>/worker.json`. "Registered" means both that profile and a
host-link identity exist; subsequent launches skip the flow.

On a TTY an onboarding screen walks three steps: name (prefilled with
`<username>@<hostname>/<ip>`), connection (creates or loads the host-link
identity, shows the address plus `@handle`, prompts for the OpenHuman owner,
where `Enter` saves and `Esc` skips), and confirm (a summary panel; `Enter`
finishes, `q`/`Ctrl-C` aborts without writing). On completion, if an owner is
set, a one-time introduction DM is sent (best-effort).

Headless or non-TTY, it auto-registers with the default name and the env
owner if there is one, warning when no owner is set, so the daemon stays
scriptable.

The profile threads through the rest of the worker: the daemon advertises the
profile name as its directory-card label (unless `--name` overrides it), and the
wrapper uses the profile owner as the final fallback in the recipient chain (any
`MEDULLA_*` env owner still wins).

## `medulla sessions`

Lists the recent Claude Code and Codex sessions found on this machine as JSON, so
a script can pick one up without parsing transcript directories itself.

```sh
medulla sessions
```

## `medulla workflow`

Author, inspect, and run [workflows](../features/workflows.md): saved multi-step
plans whose `agent` steps each run as a real harness session. The verbs split
into three groups: authoring, inspection, and execution.

```sh
medulla workflow list                  # every installed workflow
medulla workflow get <id>              # one workflow, whole
medulla workflow create <id>           # install from a document on stdin
medulla workflow delete <id>           # uninstall
medulla workflow apply-ops <id>        # apply graph patches from stdin
medulla workflow preview-ops <id>      # check those patches without saving
medulla workflow validate [id]         # validate a saved workflow, or stdin
medulla workflow catalog [kind]        # the node kinds an author may use
medulla workflow dry-run <id>          # simulate, dispatching nothing
medulla workflow run <id>              # run against the coding CLIs on this machine
medulla workflow resume <run-id>       # release approval gates and continue
medulla workflow cancel <run-id>       # stop a run executing in this process
medulla workflow list-runs <id>        # a workflow's run history
medulla workflow get-run <run-id>      # one run record
medulla workflow mcp                   # serve the workflow tools over MCP
```

| Flag | Effect |
| --- | --- |
| `--input <json>` | Trigger payload for `run` / `dry-run`. |
| `--model <name>` | The model a step runs on when it names none of its own, for this invocation. Replaces `[workflows] defaultModel`; an empty value clears it. On `defaults`, pins it in the saved workflow instead. |
| `--run-id <id>` | Id to give the run (default: a fresh one). |
| `--approve <node-id>` | Gate to release on `resume` (repeatable). |
| `--reject <node-id>` | Gate to refuse on `resume` (repeatable). |
| `--config <path>` | Explicit config file (`.toml` or `.json`). |

Workflow documents and graph ops are read from stdin, and every verb prints JSON,
so the surface is usable by a person and by an agent without either being a
special case.

`cancel` is process-local: a run started by `medulla workflow run` in one shell
cannot be cancelled from another, because there is no control channel between two
CLI invocations. The command reports that explanation. The
paths that can always cancel are the ones owning the running process: the TUI
cancels the run it started, and an orchestrator's abort frame reaches the daemon
executing it.

`medulla workflow mcp` is not for a human to run. It is the command Medulla
attaches to an ACP session so the harness on the other end can author workflows
itself.

## `medulla init`

Draft a [`MEDULLA.md`](../features/workspace-profiles.md) workspace profile, the
short, operator-editable routing file a repository root carries:

```sh
medulla init             # draft for the current directory
medulla init ./payments  # or for a specific one
medulla init --offline   # skip the model and write an editable stub
medulla init --force     # overwrite an existing profile (default: refuse)
```

`init` reads the directory's `AGENTS.md`, `CLAUDE.md`, and `README.md`, asks the
configured model to distil a summary plus routing hints, and renders the shipped
template for review. With `--offline`, no model available, a model failure, or no
source files, it writes a deterministic stub instead, so it always leaves a
usable file. It refuses to overwrite an authored profile unless `--force`/`-f` is
given. Inference resolves in one order: an explicit `OPENROUTER_API_KEY` wins,
else the backend's inference surface with your `medulla login` token.

`init` writes the file and nothing else. To also make the orchestrator aware of
the directory, use [`medulla workspace add`](#medulla-workspace).

## `medulla workspace`

Manage the workspace registry, the directories the orchestrator knows about and
can place work in:

```sh
medulla workspace add             # profile + register the current directory
medulla workspace add ./payments  # or a specific one
medulla workspace list            # show the registry (--json for machines)
medulla workspace remove .        # unregister (files and MEDULLA.md untouched)
```

`add` does everything [`init`](#medulla-init) does, then enrols the directory in
your config. Two lists are written: `[workflow].workspaces`, the roots whose
`MEDULLA.md` rides every backend session mint, and `[fleet].workspaces`, the
declared `Host -> Harness -> Workspace` chain work is placed onto. A profile that
is in neither is never read.

Both land in one file: the explicit `--config` path, else the highest-precedence
file in the layered load, else `<medulla home>/config.toml`, which is the same
file the TUI writes, so the CLI and the running app share one registry.

`add` is idempotent and safe over an existing profile: a `MEDULLA.md` that is
already there is kept and the directory is still registered, so running it after
[`medulla init`](#medulla-init) works. Re-running keeps the entry's id and
hand-tuned fields (name, harness, templates) and refreshes the rest. Pass
`--force` to redraft the `MEDULLA.md` as well.

When the workspace's harness is not already declared, `add` declares it and its
host, so the `Host -> Harness -> Workspace` chain resolves. Existing
declarations are never rewritten.

The registry is written as TOML. A `--config` path ending in `.json` is refused
before anything is written.

| Flag | Effect |
| --- | --- |
| `--harness <id>` | Attach the workspace to this harness instead of the first declared one (`add`). |
| `--force`, `-f` | Overwrite an existing `MEDULLA.md` (`add`). |
| `--offline` | Skip the model call and write the editable stub (`add`). |
| `--json` | Emit JSON instead of human-readable output (`list`). |
| `--config <path>` | Explicit config file (`.toml` or `.json`) holding the registry. |

## `medulla hub`

Relay hosted-backend tasks to configured host-link workers:

```sh
medulla hub
```

The hub is the outbound half of the peer harness plane. It takes a task the hosted
backend produced for a registered worker, encodes a `medulla-task/1` task
frame, sends it in an encrypted Signal DM, and correlates the worker's
status/reply/error frames back to the originating dispatch, routing each reply to
exactly one waiting dispatch, since concurrent tasks share a destructively drained
inbox. It requires backend credentials plus a configured worker roster and fails
with an actionable message when there is nothing to run.

## `medulla update`

Prebuilt releases self-update from GitHub:

```sh
medulla update           # download, verify (sha256), and install the latest release
medulla update --check   # only report whether a newer version is available
```

`update` downloads the platform asset named in the release's `latest.json`
manifest, verifies its SHA-256, extracts the binary, and atomically replaces the
running executable (the previous binary is kept as `<exe>.old` for rollback). It
refuses when the executable path isn't writable, for example a system-managed
install; use your package manager there.

The TUI also runs a background check ~10s after startup and every 6h, surfacing
an "update vX.Y.Z available" banner in the header. Disable it with `[update]
check = false` or the `MEDULLA_NO_UPDATE_CHECK=1` env var; point the checker at a
different manifest with `MEDULLA_UPDATE_URL`.
