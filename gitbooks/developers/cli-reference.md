# CLI Reference

The `medulla` binary is both the terminal app and a small suite of subcommands
for headless operation, bridging coding-agent harnesses to
[tiny.place](https://tiny.place), and self-updating.

| Command | What it does |
| --- | --- |
| `medulla` | Bare invocation starts the [TUI](#the-tui). |
| `medulla login` / `logout` | [Browser OAuth login](authentication.md); clears credentials. |
| `medulla daemon` | [Headless coding-agent daemon](#medulla-daemon) over tiny.place. |
| `medulla codex` / `claude` / `opencode` | [Harness wrappers](#harness-wrappers): run a CLI, bridged to tiny.place. |
| `medulla sessions` | List recent claude/codex sessions as JSON. |
| `medulla update` | [Self-update](#medulla-update): download, verify, install the latest release. |
| `medulla version` / `help` | Version string; usage. |

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
| `--core` | Drive the [core orchestration server](configuration.md#core-socket) over its Unix socket. |
| `--no-alt-screen` | Stay on the main screen buffer (useful for scrollback while debugging). |

The core runtime unlocks the Workers tab (fleet peer management) and task
steering (`X` cancel task, `A` answer a pending question).

## `medulla daemon`

A headless coding-agent daemon that serves
[claude](https://www.anthropic.com/claude-code),
[codex](https://github.com/openai/codex), and
[opencode](https://github.com/sst/opencode) over encrypted tiny.place DMs. On
first launch it runs a one-time [worker registration](#first-run-worker-registration)
flow. `medulla daemon --reonboard` forces that flow again.

## Harness wrappers

`medulla codex` / `medulla claude` / `medulla opencode` launch the real
coding-agent CLI in your terminal exactly as if you had run it directly —
inherited stdio, unrecognized flags passed through verbatim — while bridging the
session to tiny.place underneath. The wrapper tails the harness's own JSONL
transcript, normalizes each record into a typed `SessionEnvelopeV2` event, and
forwards the stream as encrypted [Signal-protocol](https://signal.org/docs/) DMs
to the configured owner; with inbound input enabled it also polls for
owner→session control frames and types their text into the child.

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
writer. stdio is inherited (no PTY): for a pristine full-screen TUI, run without
inbound input (or `--no-bridge`) so stdin stays attached to the terminal —
enabling input injection pipes stdin as a best-effort byte pump. `medulla
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
