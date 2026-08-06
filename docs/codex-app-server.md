# `codex-server`: Codex on a shared process

`codex-server` runs a task as a *thread* on a long-lived `codex app-server`
instead of forking `codex exec` for it. One process serves every lane, so a
fan-out of ten costs one Codex runtime rather than ten.

It is a **flavor** of Codex, not a separate harness. Credentials, `codexOverrides`
presets, the inference endpoint, attribution and the seat the tokens bill to are
all resolved from `codex` exactly as they are for a CLI run. The only thing that
changes is how the process is driven.

## When to use it

Use it when several tasks run at once and you care how long they take to start —
which is to say, for workflows. A CLI fork pays the whole Codex startup on every
task: process spawn, config load, MCP server handshakes, tool discovery. On a
graph with ten `agent` nodes, that is ten of everything, and the cost grows with
the fan-out.

Use plain `codex` when you want to *watch* a lane work. The app-server reports
structured thread items rather than the JSONL stream Medulla's mappers were
written against, and this transport folds only the parts that answer "is it
working, what did it say, what did it cost". Per-step tool calls, reasoning, file
edits and command output do not reach the agent rail. That trade is the point:
you get throughput, you give up the picture.

## Selecting it

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

For a caller with no frame to state it on — a wrapper, a locally launched
harness — there is an environment switch, mirroring the ACP one:

```sh
export MEDULLA_HARNESS_TRANSPORT=app-server
```

A worker advertises the flavor in its capabilities (`harnessFlavors`) only when
it has Codex at all. A task frame naming a flavor the selected provider cannot
run is **refused**, not downgraded: an operator who asked for the shared process
and silently got a CLI fork has no way to notice.

The one exception is local workflow dispatch, which drops the flavor when the
provider itself fell back — for the same portability reason a graph authored
against `codex` still runs on a worker that only has `claude`.

## How processes are shared

Connections are pooled per *process identity*: the resolved `codex` binary, any
pinned argv, and the environment that decides who the process authenticates as
(`CODEX_HOME`, `CODEX_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`,
`OPENAI_ORG_ID`, `OPENAI_PROJECT`, and the proxy). Two tasks that agree on all of
that share one process; two that disagree on any of it get their own, because a
process authenticates once and holds that identity for its whole life.

The pinned argv is the same process-level Codex configuration the CLI seam
assembles: `MEDULLA_CODEX_ARGS` / `TINYPLACE_CODEX_ARGS`, the daemon's configured
extra args, and a routed preset's `codexOverrides` — the provider block, the
API-key auth preference and the model catalog entry without which Codex prefers a
signed-in ChatGPT account and never consults the routed endpoint. Because all of
it is in the key, a routed run and a plain one never land on the same process.

Everything per-task is a thread or turn parameter, which the protocol scopes
correctly:

| Per task (thread/turn)                | Per process (pool key)   |
| ------------------------------------- | ------------------------ |
| working directory                     | `codex` binary and argv  |
| model                                 | `CODEX_HOME`             |
| sandbox and approval policy           | credentials and endpoint |
| the prompt, and the whole transcript  |                          |

**Two threads in one process do not see each other's context.** A task frame is
discrete work and gets its own thread, exactly as a CLI run gets its own process.

A pooled process that dies is discarded and replaced on the next task, rather
than being reaped in the background: that is the only moment the answer matters.

## Permissions

The operator's single skip-permissions consent maps onto the thread the same way
it maps onto the CLI:

| Consent  | Sandbox              | Approvals    | Server-initiated approvals |
| -------- | -------------------- | ------------ | -------------------------- |
| Granted  | `danger-full-access` | `never`      | accepted                   |
| Withheld | `workspace-write`    | `on-request` | declined                   |

Codex's default `workspace-write` cannot serve a delegated task: the git
directory of a linked worktree sits outside the worktree, so `git commit` cannot
write its refs, and with the network off `git push` and `gh` fail outright. That
is why consent maps to full access here as it does for `codex`.

Requests the client cannot answer honestly — dynamic tool calls, MCP
elicitations, user-input prompts — are refused rather than guessed at: no
operator is watching a delegated task.

## Aborts and stalls

An abort or an idle timeout sends `turn/interrupt` for that thread. It never
kills the process, because the process is not this task's to kill — other lanes
are running on it. Killing the child was how the CLI transport ended a run; here
it would be a bug.

## Limits

- **No follow-up input.** `input` frames are refused for an app-server task, as
  they are for ACP. The transport has a steering operation; Medulla does not use
  it yet.
- **Minimal event detail.** See "When to use it" above.
- **No Medulla hooks.** The CLI seam installs them as a per-run `-c hooks=…`
  override paired with a capability grant in the child's environment. Neither
  scopes to a *shared* process: a hook installed for one lane would fire on every
  other lane's turns, and the grant it redeems is stripped for the same reason a
  fleet grant is. A run that configured hooks logs a warning rather than letting
  the operator believe they are running.
- **Codex only.** No other harness ships an app-server. Pairing the transport
  with another provider is refused at every layer that can see both.
- **Experimental upstream.** `codex app-server` is marked experimental by the
  Codex CLI. The client speaks a deliberately small subset — `initialize`,
  `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`, and a handful
  of notifications — to keep the surface that can break small.
