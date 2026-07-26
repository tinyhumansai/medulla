# CLI Reference

The `medulla` binary is both the terminal app and a small suite of subcommands
for headless operation, bridging coding-agent harnesses to
[tiny.place](https://tiny.place), and self-updating.

| Command | What it does |
| --- | --- |
| `medulla` | Bare invocation starts the [TUI](#the-tui). |
| `medulla run <instruction>` | [Headless one-shot](#medulla-run): submit one instruction to a core socket and stream events as JSON lines. |
| `medulla login` / `logout` | [Browser OAuth login](authentication.md); clears credentials. |
| `medulla daemon` | [Headless coding-agent daemon](#medulla-daemon) over tiny.place (`--tui` for the operator screen). |
| `medulla codex` / `claude` / `opencode` | [Harness wrappers](#harness-wrappers): run a CLI, bridged to tiny.place. |
| `medulla sessions` | List recent claude/codex sessions as JSON. |
| `medulla memory <cmd>` | [Persona memory](#medulla-memory): `status` / `ingest` / `backfill` / `compile` / `search`. |
| `medulla init [dir]` | [Draft a MEDULLA.md](#medulla-init) workspace profile. |
| `medulla workspace <cmd>` | [Workspace registry](#medulla-workspace): `add [dir]` / `list` / `remove <dir\|id>`. |
| `medulla hub` | [Relay hosted-backend tasks](#medulla-hub) to configured tiny.place workers. |
| `medulla update` | [Self-update](#medulla-update): download, verify, install the latest release. |
| `medulla version` / `help` | Version string; usage. |

Unknown first arguments are treated as arguments to the main TUI rather than
rejected as unknown subcommands.

## The TUI

A [ratatui](https://ratatui.rs/) terminal UI over the SDK: chat with the
orchestrator and watch agent lanes, traces, and context live. On startup it
selects one of three [runtimes](configuration.md#runtimes) — core socket, backend
HTTP/SSE, or mock — and falls back down that chain if a preferred one is
unavailable, showing why in the status line.

TUI flags:

| Flag | Effect |
| --- | --- |
| `--config <path>` | Explicit config file (`.toml` or `.json`); bypasses layered discovery. |
| `--core-socket <path>` | Attach to a running [core orchestration server](configuration.md#core-socket) at that Unix socket. |
| `--mock` | Force the scripted offline runtime and skip login. |
| `--no-alt-screen` | Stay on the main screen buffer (useful for scrollback while debugging). |

The core runtime unlocks the Routing tab (fleet peer management) and task
steering (`X` cancel task, `A` answer a pending question).

## `medulla run`

A headless, scriptable path to a single instruction. `medulla run` drives the
same [core-socket runtime](configuration.md#core-socket) the TUI uses, waits for
the socket to become usable, submits one instruction, and streams the resulting
events as newline-delimited JSON:

```sh
medulla run "audit the payments service for unbounded retries"
medulla run --core-socket /tmp/medulla.sock "..."   # explicit socket
```

Everything after the flags is joined into one instruction; with no instruction it
errors. It retains the cycle receipt the socket returns, emits each folded event
as a JSON line, ignores unrelated cycle completions, and returns only when the
correlated cycle ends — so a script gets a deterministic, non-TUI path with no
separate wire protocol to learn. Attach failures, rejections, an unavailable
runtime, and completion timeouts are all reported.

## `medulla daemon`

A headless coding-agent daemon that serves
[claude](https://www.anthropic.com/claude-code),
[codex](https://github.com/openai/codex), and
[opencode](https://github.com/sst/opencode) over encrypted tiny.place DMs. On
first launch it runs a one-time [worker registration](#first-run-worker-registration)
flow. `medulla daemon --reonboard` forces that flow again.

When stdout is a terminal it selects the operator UI unless headless behaviour is
explicitly requested; a non-terminal launch runs headless, and `--tui` forces the
operator screen. The operator UI opens by asking two things before it listens for
work: an execution mode (Interactive holds live, watchable harness sessions;
Headless launches one process per task and shows logs) and a default harness from
the providers detected on `PATH`. It then presents four tabs — **Agents** (live
sessions with an embedded PTY you can attach to or take over), **Master** (the
worker address, saved masters, and operator messages), **Workspaces** (the
canonicalized roots the worker may advertise), and **Requests** (pending contact
requests to accept, decline, or block).

Daemon flags:

| Flag | Effect |
| --- | --- |
| `--tui` | Force the operator screen (default on a terminal). |
| `--providers <a,b>` | Restrict the accepted harnesses. |
| `--workspace <dir>` | Set the primary task working directory. |
| `--handle <name>` | Register a tiny.place `@handle`. |
| `--model <name>` | Supply a default model hint. |
| `--concurrency <n>` | Cap simultaneous executions. |
| `--once` | Drain the current inbox once and exit. |
| `--no-onboard` | Skip publishing/registration onboarding. |
| `--reonboard` | Replace the stored worker registration. |
| `--dangerously-skip-permissions` | Pass the provider's unsafe permission switch. |
| `--no-trust-workspace` | Do not pre-trust a Claude workspace in the TUI path. |

The primary `--workspace` is the execution directory; any additional advertised
roots are capability metadata, not per-task working directories. Skip-permission
mode is opt-in and visibly named for a reason — it hands the harness the
provider's unsafe switch.

## Harness wrappers

`medulla codex` / `medulla claude` / `medulla opencode` launch the real
coding-agent CLI in your terminal exactly as if you had run it directly —
unrecognized flags passed through verbatim — while bridging the
session to tiny.place underneath. The wrapper tails the harness's own JSONL
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
medulla codex --no-bridge       # pure passthrough: run the CLI with no tiny.place bridge
medulla codex -- --no-bridge    # `--` forces everything after it to the child
```

Configuration is by environment variable (mirroring the tinyplace CLI):

| Variable | Effect |
| --- | --- |
| `TINYPLACE_HARNESS_DM_TO` / `TINYPLACE_<P>_DM_TO` / `TINYPLACE_OPENHUMAN_OWNER` | tiny.place owner to forward the session envelopes to. |
| `TINYPLACE_HARNESS_RECEIVE_FROM` / `TINYPLACE_<P>_RECEIVE_FROM` | Peer whose input control frames / plain DMs are injected (defaults to the owner). |
| `TINYPLACE_HARNESS_RECEIVE=0` / `TINYPLACE_<P>_RECEIVE=0` | Disable inbound input injection. |
| `TINYPLACE_<P>_BIN` (`TINYPLACE_CODEX_BIN`, `TINYPLACE_CLAUDE_BIN`, `TINYPLACE_OPENCODE_BIN`) | Override the provider binary. |
| `TINYPLACE_<P>_SESSIONS_DIR` | Override the transcript directory the tailer watches. |

If no owner is configured (and `--no-bridge` was not passed), the wrapper prints
a single warning and runs as a plain passthrough.

**Scope notes.** This is the single-terminal `--raw` wrapper. It does not build
the tinyplace TUI chrome, the `--agent` plugin mode, the machine-bus
multi-terminal coordination, the opencode SSE server, or the terminal-envelope
writer. `medulla
opencode` runs as a passthrough with input injection but no transcript tailing
(its session log is not a flat JSONL the mappers read).

## First-run worker registration

The first time a worker starts — `medulla daemon`, or a bridged `medulla
codex|claude|opencode` — it runs a one-time onboarding flow that names the worker
and connects it to an owner, then persists a small profile at
`<medulla-home>/worker.json`. "Registered" means both that profile and a
tiny.place identity exist; subsequent launches skip the flow.

* **On a TTY** an onboarding screen walks three steps: **name** (prefilled with
  `<username>@<hostname>/<ip>`), **connection** (creates/loads the tiny.place
  identity, shows the address + `@handle`, prompts for the OpenHuman owner —
  `Enter` saves, `Esc` skips), and **confirm** (a summary panel; `Enter`
  finishes, `q`/`Ctrl-C` aborts without writing). On completion, if an owner is
  set, a one-time introduction DM is sent (best-effort).
* **Headless / non-TTY** it auto-registers with the default name and the env
  owner (if any), warning when no owner is set, so the daemon stays scriptable.

The profile threads through the rest of the worker: the daemon advertises the
profile name as its directory-card label (unless `--name` overrides it), and the
wrapper uses the profile owner as the final fallback in the recipient chain (any
`TINYPLACE_*` env owner still wins).

## `medulla memory`

Manage the [persona-memory](architecture.md#persona-memory) layer that turns local
coding-agent history into a durable, prompt-ready persona pack:

```sh
medulla memory status                # print the memory-layer status
medulla memory ingest                # incremental ingest pass (LLM-backed; needs an API key)
medulla memory backfill              # full backfill ingest pass (LLM-backed; needs an API key)
medulla memory compile               # recompile the pack from persisted trees (offline)
medulla memory search "<query>"      # BM25 search over the persona corpus (offline)
```

| Flag | Effect |
| --- | --- |
| `--json` | Emit JSON instead of human-readable output. |
| `--facet <name>` | Restrict a `search` to one facet. |
| `--k <n>` | Cap `search` results (default 5). |
| `--config <path>` | Explicit config file (`.toml` or `.json`) for the memory section. |

`ingest` and `backfill` call an LLM and need an API key; `status`, `compile`, and
`search` run fully offline against the persisted corpus.

## `medulla init`

Draft a [`MEDULLA.md`](../features/workspace-profiles.md) workspace profile — the
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
source files, it writes a deterministic stub instead — so it always leaves a
usable file. It refuses to overwrite an authored profile unless `--force`/`-f` is
given. Inference resolves the same way [memory](#medulla-memory) does: an explicit
`OPENROUTER_API_KEY` wins, else the backend's inference surface with your
`medulla login` token.

`init` writes the file and nothing else. To also make the orchestrator aware of
the directory, use [`medulla workspace add`](#medulla-workspace).

## `medulla workspace`

Manage the workspace registry — the directories the orchestrator knows about and
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
file in the layered load, else `<medulla home>/config.toml` — the same file the
TUI writes, so the CLI and the running app share one registry.

`add` is idempotent and safe over an existing profile: a `MEDULLA.md` that is
already there is kept and the directory is still registered, so running it after
[`medulla init`](#medulla-init) works. Re-running keeps the entry's id and
hand-tuned fields (name, harness, templates) and refreshes the rest. Pass
`--force` to redraft the `MEDULLA.md` as well.

When the workspace's harness is not already declared, `add` declares it and its
host, so the `Host -> Harness -> Workspace` chain actually resolves. Existing
declarations are never rewritten.

The registry is written as TOML: a `--config` path ending in `.json` is refused
up front rather than corrupted.

| Flag | Effect |
| --- | --- |
| `--harness <id>` | Attach the workspace to this harness instead of the first declared one (`add`). |
| `--force`, `-f` | Overwrite an existing `MEDULLA.md` (`add`). |
| `--offline` | Skip the model call and write the editable stub (`add`). |
| `--json` | Emit JSON instead of human-readable output (`list`). |
| `--config <path>` | Explicit config file (`.toml` or `.json`) holding the registry. |

## `medulla hub`

Relay hosted-backend tasks to configured [tiny.place](https://tiny.place) workers:

```sh
medulla hub
```

The hub is the outbound half of the peer harness plane. It takes a task the hosted
backend produced for a registered worker, encodes a `medulla-tinyplace/1` task
frame, sends it in an encrypted Signal DM, and correlates the worker's
status/reply/error frames back to the originating dispatch — routing each reply to
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
refuses when the executable path isn't writable (e.g. a system-managed install) —
use your package manager there.

The TUI also runs a background check ~10s after startup and every 6h, surfacing
an "update vX.Y.Z available" banner in the header. Disable it with `[update]
check = false` or the `MEDULLA_NO_UPDATE_CHECK=1` env var; point the checker at a
different manifest with `MEDULLA_UPDATE_URL`.
