---
description: >-
  How a coding harness plugs into Medulla: the public wire contract, the ACP
  stdio transport, and the shared-process Codex path.
---

# Harness integration

Three separate things sit between Medulla and a coding-assistant CLI, and they
are independent of each other:

* The wire contract, a set of JSON shapes a backend-fronted harness sends to this
  client, mirrored as serde types in the SDK.
* The ACP transport, which is how the daemon launches and drives Claude Code,
  Codex, and OpenCode over stdio.
* `codex-server`, a Codex flavor that runs tasks as threads on one long-lived
  `codex app-server` process instead of forking a CLI per task.

## The wire contract

An agent harness is a long-lived agent that accepts natural-language
instructions, keeps a durable task board, delegates to connected agents, and
surfaces a status snapshot plus an event stream. When a backend fronts that
harness, its JSON crosses the wire to this client. The SDK represents those
public wire shapes as serde types so the SDK and TUI decode them consistently.

The mirrors live in
[`medulla::harness_contract`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_contract/). Field names
match the public JSON contract: every struct is
`#[serde(rename_all = "camelCase")]` and the status and state enums are
lowercase. Round-trip tests in `harness_contract/tests.rs` assert those names
against hand-written JSON literals. The format is versioned as a wire contract so
clients can validate compatibility independently of any implementation.

### Mirrored types

| Rust type | Notes |
| --- | --- |
| `TrackedTask`, `TrackedTaskStatus` | Status is `open`, `active`, `blocked`, `done`, `cancelled`. `createdAt` and `updatedAt` are ISO-8601 strings; `delegatedTaskIds` and `notes` are always-present arrays. |
| `HarnessStatus`, `HarnessState`, `HarnessUsage` | State is `idle`, `running`, `stopped`. `lastResult` is an opaque cycle-result payload (`serde_json::Value`). |
| `HarnessEvent` | Tagged by `kind`. The three lifecycle kinds (`instruction_queued`, `cycle_start`, `cycle_end`) are distinct variants; `task_board_changed` carries a `TrackedTask`; `cycle_event` wraps an opaque event payload. |
| `InstructionReceipt` | The serialisable `{ instructionId, cycleId }` fields. |
| `AgentBudgetMetadata` | The `metadata.budget` stamp on an agent descriptor. |
| `SeatHeadroom`, `WindowHeadroom` | Live seat headroom with a per-window map. |

### The two opaque payloads

`HarnessStatus::last_result` (`CycleResult`) and
`HarnessEvent::CycleEvent { event }` (`CycleEvent`) are kept as
`serde_json::Value` rather than mirrored field by field. Neither is a contract
this client consumes; keeping them opaque preserves them losslessly without
coupling the client to their internals.

### Timestamps in `AgentBudgetMetadata` and `SeatHeadroom`

Both describe seat headroom, but the public contract formats their timestamps
differently. `SeatHeadroom` carries epoch-milliseconds numbers throughout
(`primaryResetsAt`, `throttledUntil`, and each window's `resetsAt`).
`AgentBudgetMetadata`, the roster-facing stamp the backend writes onto a
descriptor, carries `primaryResetsAt` as an ISO-8601 string, formatted at the
roster boundary.

### Reserved tool names

The harness composes its own memory and task-tracker modules eagerly, so a
business tool that reuses one of their names throws at construction. A
third-party module author must avoid these six names, exported as
`harness_contract::RESERVED_TOOL_NAMES` with an `is_reserved_tool_name` helper:

```
task_create   task_update   task_list
memory_write  memory_read   memory_list
```

These names are part of the public harness contract.

### What the TUI renders

The Sessions tab renders two harness surfaces, both additive and both degrading to
nothing when their payload is absent.

The task board appears when the backend runtime surfaces a `HarnessStatus`
(`RuntimeSnapshot::harness`). The Sessions transcript header then shows a compact
board: a per-status count summary (`tasks · open 2 · active 1 · done 3`) followed
by one `glyph title` row per task. The pure helpers live in
[`medulla::ui::harness`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/harness/).

The seat budget appears when a selected lane's agent descriptor carries a
`metadata.budget` stamp (`AgentBudgetMetadata`). The header then shows a one-line
note, `seat Claude Max 5x · 1.2M left`, or the same line ending in `exhausted`
when the seat is spent.

Budget display is strictly read-only. Seat CRUD (connecting, enabling, or
removing a user's own subscription seat) stays a backend REST concern and is not
built into the TUI. The TUI only decodes and shows the `metadata.budget` stamp
the backend already attaches to a descriptor.

## The ACP transport

Medulla can communicate with coding harnesses through version 1 of the
[Agent Client Protocol](https://agentclientprotocol.com/). This gives the daemon
one lifecycle and event stream for Claude Code, Codex, OpenCode, and future ACP
agents instead of teaching the orchestration layer each harness's private JSONL
format.

Set the protocol in the daemon environment:

```sh
export MEDULLA_HARNESS_PROTOCOL=acp
medulla daemon --headless
```

The daemon launches these ACP servers over stdio:

| Harness | ACP server |
| --- | --- |
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp@latest` |
| Codex | `npx -y @agentclientprotocol/codex-acp@latest` |
| OpenCode | `opencode acp` |

The ACP client performs `initialize`, creates or loads a session, sends
`session/prompt`, streams `session/update` notifications into Medulla status
events, answers permission requests, and sends `session/cancel` when a remote
task is aborted. Permission requests are denied unless the daemon was started
with its existing skip-permissions option.

The legacy provider JSONL transport remains the default during migration.
Removing `MEDULLA_HARNESS_PROTOCOL` returns to it immediately, which permits a
host-by-host rollout without changing the Medulla task-frame protocol.

ACP session ids are returned through the same `RunTaskResult::session_id` field
used by the legacy adapters, so conversation resumption does not depend on a
provider-specific transcript shape.

## Codex on a shared process

`codex-server` runs a task as a thread on a long-lived `codex app-server`
instead of forking `codex exec` for it. One process serves every lane, so a
fan-out of ten costs one Codex runtime rather than ten.

It is a flavor of Codex rather than a separate harness. Credentials,
`codexOverrides` presets, the inference endpoint, attribution, and the seat the
tokens bill to are all resolved from `codex` exactly as they are for a CLI run.
The only thing that changes is how the process is driven.

### When to use it

Use it when several tasks run at once and you care how long they take to start,
which in practice means workflows. A CLI fork pays the whole Codex startup on
every task: process spawn, config load, MCP server handshakes, tool discovery. On
a graph with ten `agent` nodes, that is ten of everything, and the cost grows
with the fan-out.

Use plain `codex` when you want to watch a lane work. The app-server reports
structured thread items rather than the JSONL stream Medulla's mappers were
written against, and this transport folds only the parts that answer whether it
is working, what it said, and what it cost. Per-step tool calls, reasoning, file
edits, and command output do not reach the agent rail. The transport trades that
detail for throughput.

### Selecting it

Anywhere a harness is named:

```yaml
# a workflow node, or a workflow's `defaults` block
harness: codex-server
model: gpt-5.6-terra
```

```jsonc
// a fleet task
{ "instruction": "run the migration", "harness": "codex-server" }
```

For a caller with no frame to state it on (a wrapper, or a locally launched
harness) there is an environment switch, mirroring the ACP one:

```sh
export MEDULLA_HARNESS_TRANSPORT=app-server
```

A worker advertises the flavor in its capabilities (`harnessFlavors`) only when
it has Codex at all. A task frame naming a flavor the selected provider cannot
run is refused rather than downgraded, because an operator who asked for the
shared process and silently got a CLI fork has no way to notice.

The one exception is local workflow dispatch, which drops the flavor when the
provider itself fell back, for the same portability reason a graph authored
against `codex` still runs on a worker that only has `claude`.

### How processes are shared

Connections are pooled per process identity: the resolved `codex` binary, any
pinned argv, and the environment that decides who the process authenticates as
(`CODEX_HOME`, `CODEX_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`,
`OPENAI_ORG_ID`, `OPENAI_PROJECT`, and the proxy). Two tasks that agree on all of
that share one process; two that disagree on any of it get their own, because a
process authenticates once and holds that identity for its whole life.

Everything per-task is a thread or turn parameter, which the protocol scopes
correctly:

| Per task (thread or turn) | Per process (pool key) |
| --- | --- |
| working directory | `codex` binary and argv |
| model | `CODEX_HOME` |
| sandbox and approval policy | credentials and endpoint |
| the prompt, and the whole transcript | |

Two threads in one process do not see each other's context. A task frame is
discrete work and gets its own thread, exactly as a CLI run gets its own process.

A pooled process that dies is discarded and replaced on the next task, rather
than being reaped in the background, because its health only has to be known at
the moment a task is about to use it.

### Permissions

The operator's single skip-permissions consent maps onto the thread the same way
it maps onto the CLI:

| Consent | Sandbox | Approvals | Server-initiated approvals |
| --- | --- | --- | --- |
| Granted | `danger-full-access` | `never` | accepted |
| Withheld | `workspace-write` | `on-request` | declined |

Codex's default `workspace-write` cannot serve a delegated task: the git
directory of a linked worktree sits outside the worktree, so `git commit` cannot
write its refs, and with the network off `git push` and `gh` fail outright. That
is why consent maps to full access here as it does for `codex`.

Requests the client cannot answer honestly (dynamic tool calls, MCP elicitations,
user-input prompts) are refused rather than guessed at, because no operator is
watching a delegated task.

### Aborts and stalls

An abort or an idle timeout sends `turn/interrupt` for that thread. The process
must stay alive, because other lanes are running on it, so ending a run on a
shared process is done by interrupting the turn.

### Limits

Follow-up input is unavailable. `input` frames are refused for an app-server
task, as they are for ACP. The transport has a steering operation; Medulla does
not use it yet.

Event detail is minimal, as described under
[when to use it](#when-to-use-it).

Codex is the only provider. No other harness ships an app-server. Pairing the
transport with another provider is refused at every layer that can see both.

The upstream is experimental. `codex app-server` is marked experimental by the
Codex CLI. The client speaks a deliberately small subset (`initialize`,
`thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`, and a handful of
notifications) to keep the surface that can break small.

## Read next

* [Glossary](glossary.md): harness, provider, flavor, task board.
* [Configuration](configuration.md): the `[router]` and `[[customHarnesses]]` sections.
* [Attribution and routing](attribution-and-routing.md): what happens to a harness's outbound inference traffic.
