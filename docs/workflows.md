# Workflows

A Medulla task is one instruction handed to one harness. A **workflow** is a
saved, multi-step plan: a directed acyclic graph whose `agent` steps each run as
a real coding-harness session — Claude Code, Codex, or OpenCode — in the order
and with the parallelism the graph declares.

The engine is [`tinyflows`](https://github.com/tinyhumansai/tinyflows), vendored
under `vendor/tinyflows` (see [vendoring.md](vendoring.md)). Medulla supplies the
capabilities it runs against, which is where the difference from every other host
embedding that engine lives: **an `agent` node here is a dispatched task, not a
model call.**

## Where workflows live

JSON documents, one graph per file. Authored workflows are saved in the Medulla
home beside the rest of its persistent data:

```text
<medulla home>/workflows/*.json
```

A repository may still provide defaults under `<cwd>/.medulla/workflows`.
Those are read first; a user-global workflow of the same id overlays the
repository copy, so edits never have to create untracked files in the checkout.
A malformed document costs only itself: the rest of the catalogue still loads
and the failure is reported.

Run records live under `<medulla home>/state/workflows/runs/`, and the engine's
checkpoints — what lets a paused run survive a restart — under
`state/workflows/checkpoints/`.

## Writing one

The smallest useful workflow is a trigger and a step:

```json
{
  "name": "Triage",
  "description": "look at the repo, then summarise",
  "nodes": [
    { "id": "t", "kind": "trigger", "name": "start",
      "config": { "trigger_kind": "manual" } },
    { "id": "look", "kind": "agent", "name": "Inspect the repo",
      "config": { "prompt": "list the top-level files" } },
    { "id": "sum", "kind": "transform", "name": "Summarise",
      "config": { "set": { "report": "=.item.text" } } }
  ],
  "edges": [
    { "from_node": "t", "to_node": "look" },
    { "from_node": "look", "to_node": "sum" }
  ]
}
```

Exactly one node must be a `trigger`. `medulla workflow catalog` prints the
contract for all twelve node kinds, with this host's notes layered on — read it
before writing a graph rather than guessing at field names.

Things worth knowing about script steps here — a `tool_call` with the
`medulla:shell` slug, which runs in the operator's project directory:

- `args.script` is an inline script; `args.script_path` runs a file the
  repository already has. Exactly one of the two, never both. Prefer
  `script_path` when the repository maintains the script, so the graph does not
  carry a copy that drifts from it.
- `args.language` is `shell` (the default, run with bash), `javascript`, or
  `python`. `args.cwd` narrows the directory (the workspace root by default) and
  `args.env` adds environment variables as an object of strings.
- `args.input` is handed to the script on stdin as JSON, and written to a file
  at `argv[1]` and `$MEDULLA_INPUT`.
- A non-zero exit **fails the step**, with the script's own stderr in the run
  record. A step that succeeds returns `{ output, stderr }`, where `output` is
  stdout parsed as JSON when it is JSON and a string otherwise.

```json
{
  "id": "build", "kind": "tool_call", "name": "Build",
  "config": {
    "slug": "medulla:shell",
    "args": {
      "script_path": "scripts/build.sh",
      "cwd": "crates/engine",
      "env": { "PROFILE": "release" }
    }
  }
}
```

Things worth knowing about `agent` nodes here:

- `config.prompt` is the instruction (`instruction` is accepted as an alias).
- `config.agent_ref` names the **worker** to dispatch to. Omit it to use
  `workflows.defaultWorker`; with neither, the run fails and says so.
- The harness reply is `=item.text`. If the harness replied with JSON it is
  parsed too, so `=item.json.<field>` works without an `output_parser`.
- `config.requires_approval: true` parks the run until someone approves it.
- `config.harness` chooses **what** runs the step, as distinct from `agent_ref`,
  which chooses **where**: `claude`, `codex`, `opencode`, or the id of a custom
  harness preset the worker exposes. `config.model` is the model hint.

## Choosing a harness and a model

A plan is rarely one model's worth of work: triage on something cheap,
implementation on something expensive, one step on Codex because it is better at
that step. So the choice is stated at whichever layer actually owns it, and the
most specific one wins:

| Layer | Where it is written |
| --- | --- |
| the step | `config.harness` / `config.model` on an `agent` node |
| the workflow | the document's `defaults` block |
| the host | `workflows.defaultProvider` / `workflows.defaultModel` |

```json
{
  "id": "triage",
  "defaults": { "harness": "claude", "model": "claude-opus-4" },
  "nodes": [
    { "id": "t", "kind": "trigger", "name": "start",
      "config": { "trigger_kind": "manual" } },
    { "id": "sift", "kind": "agent", "name": "Sift the queue",
      "config": { "prompt": "which of these need a human?",
                  "model": "claude-haiku-4-5" } },
    { "id": "fix", "kind": "agent", "name": "Fix the top one",
      "config": { "prompt": "fix it", "harness": "codex",
                  "model": "gpt-5-codex" } }
  ],
  "edges": [
    { "from_node": "t", "to_node": "sift" },
    { "from_node": "sift", "to_node": "fix" }
  ]
}
```

Resolution is **paired**, not field-by-field: whichever layer names the harness
also supplies the model, unless a layer above it named one explicitly. A node
that says `harness: codex` and nothing else runs Codex on Codex's own default
model rather than inheriting a Claude model id — a model chosen for one harness
is meaningless, or wrong, on another. Name both when you mean both.

A harness that is not one of the three built-in CLIs is taken as a custom
harness preset id — the ones this machine has configured are listed by
`workflow_host` and in the TUI's Routing → Harnesses screen. Whether the *worker*
that runs the step exposes that preset is only answered when it runs.

`harness` must be written plainly, never as a `=`-expression. Which binary and
which credentials run a step is a decision the graph makes, not one its data
makes — an expression there would let upstream output, including a model's own,
choose it. Authoring one is refused rather than saved. `model` may be an
expression.

`medulla workflow defaults <id>` reads the block, and the same verb with
`--harness`/`--model` sets it (an empty string clears one). The copilot does the
same over `workflow_defaults`, and `workflow_host` reports what this machine
offers.

Any config string beginning `=` is a jq expression evaluated against
`{ item, items, run, nodes }`.

## Running one

```sh
medulla workflow list                 # what is installed
medulla workflow defaults triage      # what its steps run on
medulla workflow defaults triage --harness codex --model gpt-5-codex
medulla workflow defaults triage --harness ''   # back to the host default
medulla workflow dry-run triage       # simulate: no harness, no network
medulla workflow run triage           # for real, on this machine's CLIs
medulla workflow list-runs triage     # history
medulla workflow resume <run-id> --approve review
medulla workflow cancel <run-id>   # only reaches runs in this process — see below
```

`cancel` is process-local. A run started by `medulla workflow run` in one shell
cannot be cancelled from another, because there is no control channel between two
CLI invocations; the command says so rather than reporting a bare failure. The
paths that can always cancel are the ones that own the running process: the TUI
cancels the run it started, and an orchestrator's abort frame reaches the daemon
executing it.

`dry-run` is the one to reach for while authoring. Validation catches a malformed
graph; a dry run catches a *well-formed* graph that is wired wrong — every
expression is resolved and every declared output shape satisfied, against
capability stand-ins, with nothing dispatched.

Every verb prints JSON and reads bulk input from stdin, so the command is usable
by a person and by an agent without either being a special case.

## Triggering one from your own harness

Everything above assumes you are in Medulla. The other case is an operator
sitting in their *own* Claude Code or Codex session who wants to say "babysit
this PR" and have the saved `babysit` workflow start.

Transport was never the problem: `medulla mcp` serves `workflow_run` to any MCP
client. What that session lacks is knowing the workflow exists, what it takes,
and that a tool call is how to start it. `medulla skills` writes that knowledge
into the harness's own skill directory, one file per workflow:

```sh
medulla skills install                       # every enabled workflow
medulla skills install babysit --with-mcp    # one, and attach the server
medulla skills install --dry-run             # what would change, writing nothing
medulla skills list                          # what is installed, and where
medulla skills sync --prune                  # match the store again
medulla skills uninstall babysit             # one
medulla skills uninstall --all               # every managed skill under the root
```

A bare `medulla skills uninstall` with neither an id nor `--all` lists what it
would remove and refuses: "remove everything" is one typo away from "remove this
one", and the removal is not recoverable from the output.

| flag | meaning |
| --- | --- |
| `--harness claude,codex,generic\|all` | which layouts to write (default: the ones already set up under the root) |
| `--scope user\|project` | `$HOME`, or this checkout (default: `user`) |
| `--dir <path>` | an explicit root, overriding `--scope` |
| `--with-mcp` | also register `medulla mcp` with each harness |
| `--with-commands` | also write the `/medulla-<id>` slash command |
| `--tools run\|full` | the tool surface a skill-triggered session gets (default: `run`) |
| `--prune` | on `sync`, delete skills for workflows that are gone or disabled |
| `--dry-run`, `--json` | report without writing; machine-readable output |

Claude gets `.claude/skills/medulla-<id>/SKILL.md`, Codex gets
`.codex/skills/…`, and any other harness gets `.medulla/skills/…` to point at
by hand. Generated files carry a `medulla:managed` marker line, and nothing
without that marker is ever overwritten or deleted — a collision is reported and
skipped, which means that workflow is *not* installed, and the command exits
non-zero so a wrapper notices. A file whose marker Medulla cannot fully parse
counts as someone else's for the same reason; remove it by hand to let Medulla
manage that path again. Re-running is a no-op.
Disabled workflows get no skill: a workflow that may not run should not be
advertised as runnable.

### Skills for the harnesses Medulla spawns

The tools already arrive on their own: a session Medulla starts is handed the
MCP server at `session/new` with a grant minted for it, so nothing needs
installing for an `agent` step to *call* `workflow_run`. What it lacks is the
knowledge — that `babysit` exists, and what it takes.

`--scope managed` fills that in without touching the operator's own
directories:

```sh
medulla skills install --scope managed --harness claude
```

The root is `<medulla home>/harness/`, laid out like a project root
(`.claude/skills/…`), and Medulla adds `--add-dir <that root>` when it spawns
Claude Code. Claude loads `.claude/skills/` from an added directory — a
documented exception to `--add-dir` being a file-access grant, and the reason
this works at all; the `permissions.additionalDirectories` *setting* grants
access without loading skills. The flag is added only once the root actually
holds skills, so an install nobody ran changes no argv.

Two things this deliberately does not do. It does not relocate the harness's
config directory: `CLAUDE_CONFIG_DIR` and `CODEX_HOME` move credentials and
settings along with the skills, and a session started under a fresh one is not
logged in. And it does not register an MCP server into the managed root —
`--with-mcp` there reports `already-attached`, because Medulla attaches the
server itself, per session, with a grant no config file can express.

Codex has no additional-directory flag, so `--scope managed` writes its files
but nothing points a spawned Codex session at them yet. The ACP transport is
the same story for both: Medulla drives `claude-agent-acp` over stdio and does
not control the underlying CLI's argv, so this applies to the direct spawn path
only.

### Attaching the server

`--with-mcp` registers `medulla mcp` alongside the skills, because a skill whose
tool is missing just produces a session confidently reaching for something that
is not there. Registration is a config merge, not a subprocess: `.mcp.json` for
Claude at project scope, `~/.codex/config.toml` for Codex, both preserving every
other server and key. Claude at user scope and the generic target have no file
we can safely write, so the command prints the exact `claude mcp add` line for
you to run instead of reporting a success Claude would never read.

### What the model then does

The skill's frontmatter description is the workflow's own description plus an
explicit "use when" clause, which is what a paraphrased request matches against.
The body is the call, built from the workflow's declared inputs:

```json
mcp__medulla__workflow_run
{ "id": "babysit", "inputs": { "pr": "<pr>", "repo": "tinyhumansai/medulla" } }
```

with a table of every input — type, whether it is required, its default — and an
instruction to ask for a missing required input rather than invent one. Inputs
that have defaults appear filled in; the rest are type-shaped placeholders, not
plausible-looking guesses. The body also says what to do when the tool is
absent, which is a normal state for a skill file that outlives an MCP
configuration: do not claim the run started, fall back to
`medulla workflow run <id> --inputs '…'`, or attach the server.

### Run-only tools, by default

`--with-mcp` writes `MEDULLA_WORKFLOW_TOOLS=run` into the registration. That is
a third tool mode beside `full` and `propose`, and
it serves exactly six verbs — `workflow_list`, `workflow_get`,
`workflow_dry_run`, `workflow_run`, `workflow_runs`, `workflow_run_get`.
Authoring, deletion, defaults, the journal, and the proposal verbs are withheld
from `tools/list` *and* refused by `tools/call`, with a refusal that says where
those things happen instead.

It is the default because a skill installed into your everyday harness hands
*every* turn in that harness whatever surface the server exposes. A turn that
came to trigger `nightly-sweep` has no business being able to rewrite or delete
it, and an unrelated turn three hours later has less. Unlike `propose`, which
denies a list of verbs, `run` allows a list: a verb added to the family later
stays withheld until someone decides a trigger-only session needs it. `--tools
full` opts back in for a session you actually want authoring from.

### The blocking call

`workflow_run` returns only when the run finishes. For a short workflow that is
fine and the generated body says to expect minutes. For a `babysit`-class
workflow it is an hour-long tool call, and most MCP clients will time out or the
operator will interrupt — at which point the run is still going but its outcome
is no longer observable from the harness.

That is a real limitation, not a rough edge. The fix is a `workflow_start` verb
returning `{ runId }` as soon as the run is admitted, with the skill body
becoming start → report the id → poll `workflow_run_get`. **It is not built
yet.** Until it is, prefer skills for workflows that finish inside a tool call,
and start the long ones from Medulla or `medulla workflow run`, where nothing is
waiting on a response.

## In the TUI

Workflows is a top-level tab: a sidebar, a canvas, and a copilot.

- **The sidebar** lists the installed workflows, with the selected one's runs
  indented beneath it. It behaves like the Settings and Routing navs — `↑↓` walk
  it, `1`-`9` jump, `Enter` opens the graph, `Esc` comes back.
- **The canvas** draws the selected workflow's graph: a box per node, laid out
  left to right by how far each step is from the trigger, a lane per concurrent
  branch, and the branch's port name written on the wire that carries it. `←→`
  follows edges, `↑↓` walks the lanes of a branch, and `i` expands the strip
  below into the selected node's whole declaration. Selecting a run in the
  sidebar overlays it: each box is recoloured by how that run left it, the steps
  it never reached are dimmed, and the inspector shows the node's duration and
  any diagnostics.
- **The copilot** (`c`) is a conversation that edits the graph. Ask for a change
  in plain words; a real harness session makes it with the MCP tools below, and
  the graph is then re-read from the store so the transcript reports what
  actually changed rather than whatever the agent said it did.

`x` runs the selected workflow and `d` simulates it — from either pane, since
both are questions about the workflow rather than about what is focused. `r`
re-reads the store. `Tab` is left alone: it walks the top-level views, here as
everywhere else.

## How an agent authors one

Medulla drives Claude Code and Codex over ACP as a *client*, so it cannot hand
them tools directly — it offers them. Every ACP session gets an MCP server
(`medulla workflow mcp`, this same binary) exposing:

| tool | what it does |
| --- | --- |
| `workflow_list` / `workflow_get` | read the catalogue |
| `workflow_catalog` | the node-kind contracts, with host notes |
| `workflow_create` | install from a whole document |
| `workflow_apply_ops` / `workflow_preview_ops` | edit by graph patch |
| `workflow_validate` | check a saved or unsaved graph |
| `workflow_dry_run` | simulate without dispatching |
| `workflow_runs` | run history |

`workflow_apply_ops` is the one that matters for editing. Rewriting a whole
document loses whatever the model misremembered; a patch is checked op by op, and
a batch that fails anywhere leaves the workflow untouched. One sharp edge:
`rename_node` rewires edges but does **not** rewrite `=nodes.<id>` expressions
inside other nodes' configs.

## How the orchestrator runs one

Two additions to the existing task protocol, both optional and backward
compatible:

- A worker's capability probe now advertises `workflows` — the ids it has
  installed, with names, descriptions, and step counts.
- A task frame may carry a `workflow` field. Naming one makes the worker run that
  saved graph instead of handing the frame's `text` to a harness; the text
  becomes the trigger payload. The ack, the reply, the correlation, and the
  work-snapshot attachment are all the ordinary ones, so an orchestrator that
  knows nothing about workflows still sees a task it dispatched and a task that
  answered.

Aborting the task cancels the run: the frame's task id is the run id, and every
node dispatched by that run carries it as its `abort_id`.

## How the hosted orchestrator sees them

The section above is the tiny.place peer contract: one hub, one worker, one task
frame. The *cloud* plane is the other direction — the hosted brain in the backend
asking this machine what it has, over the same Socket.IO connection the hub
already holds for `medulla:task_run`.

Four `medulla:*` events carry it:

| event | direction | what it says |
| --- | --- | --- |
| `medulla:register_workflows` | hub → backend | `{ workflows: [...], agentId? }` — every installed graph as an advert: id, name, description, step count, whether it is enabled, what triggers it |
| `medulla:workflow_request` | backend → hub | `{ requestId, op, workflowId?, kind?, instruction? }` where `op` is `get`, `node_kinds`, `runs`, or `copilot` |
| `medulla:workflow_result` | hub → backend | `{ requestId, ok, data?, error? }` — `data` is this host's own JSON, passed through the backend unparsed |
| `medulla:task_run` with `workflow` | backend → hub | the delegation itself, exactly as described above |

The adverts go up on every connect *and* reconnect, beside the worker roster: the
backend keys them to the socket that sent them and drops the whole entry when
that socket goes, so a hub that re-registered only its agents would come back
invisible. They are re-sent after a successful `copilot` turn, which is the one
request that changes what this host holds.

Everything is served from the same layered store the Workflows tab, the
`medulla workflow` subcommand and the MCP tools read — a socket `get` and
`medulla workflow get` are one implementation, so they cannot drift. `copilot` is
not a read: it is a whole authoring turn on this machine's own harness, with the
`medulla-workflows` tools attached, and its result is derived from re-reading the
store afterwards rather than from what the model said it did.

Three properties are load-bearing rather than incidental:

- **Every request is answered.** The backend correlates by `requestId` and waits
  on a deadline — ten seconds for a read, ten *minutes* for a copilot turn — so a
  dropped request is not a cheap no-op, it burns that whole window. An unknown
  `op` from a newer backend, a frame this build cannot decode (the `requestId` is
  recovered from the raw JSON), a missing `workflowId`, an unknown workflow, a
  store that fails, even a store that *panics*: all of them come back as
  `ok: false` with a sentence the orchestrator can render as a tool error. The
  only unanswerable frame is one carrying no `requestId` at all.
- **Nothing long-running blocks the socket.** Reads run on a blocking thread and
  a copilot turn on its own task, because awaiting either inside the Socket.IO
  callback starves engine.io's ping/pong — the backend then drops the hub, and
  every later delegation fails with "no harness connected" while the process
  still looks alive.
- **A host with workflows disabled advertises none.** The bridge is a view of the
  store and applies no policy; the refusal lives in the run path. Advertising
  graphs this host would decline to run would only teach the orchestrator to
  delegate work that bounces.

## Progress

A run reports itself in the *existing* `harness_work` vocabulary — a
`plan_update` naming every node, `todo_update` as steps settle, `subagent_start`
per agent node, and a `run_result`. So a workflow renders through the same pane
that shows a harness's own todo list, with no rendering code of its own.

## Configuration

The `workflows` section. Local code execution defaults **on** so authored
workflows run without a host bootstrap step. Outbound HTTP and third-party tools
remain deny-by-default.

```toml
[workflows]
enabled = true
defaultWorker = ""        # where agent nodes with no agentRef go
defaultProvider = ""      # claude | codex | opencode
defaultModel = ""
allowCode = true          # set false for untrusted workflows; there is no sandbox
toolAllowlist = []        # beyond the built-in medulla:* tools
httpAllowlist = []        # a bare domain also permits its subdomains
runTimeoutSecs = 600
```

Note this is `workflows`, plural. The older `workflow` key is unrelated — despite
the name it is only a list of workspace roots.

Two guards are not configurable:

- **Loopback and private addresses** are refused for `http_request` whatever the
  allowlist says — checked both by literal *and* against every address the host
  resolves to, so an allowlisted name answering `127.0.0.1` is caught. Redirects
  are not followed, since only the first URL is ever checked. A name that cannot
  be resolved is refused rather than attempted.
- **`code` nodes have no sandbox** on this host, so the default grants a
  workflow author the daemon's own privileges. Set `allowCode = false` before
  loading untrusted workflows; the refusal message says as much.
- **A script step's paths stay in the workspace.** `medulla:shell`'s
  `args.script_path` and `args.cwd` are resolved inside the configured workspace
  and refused anywhere else: relative only, no `..`, and re-checked after
  symlinks are followed, so a link inside the workspace cannot point out of it.
  A host with no workspace configured refuses both, leaving `args.script`. This
  bounds *which file runs and where* — it is not a sandbox, and nothing bounds
  what a script does once it has started.

Workflow ids and run ids both become filenames and are validated as single path
components before use: a document's `id` overrides the caller's, and a run id can
arrive on a task frame from a peer, so neither is trusted to stay inside its
directory.

Credentials never appear in a graph: a node names one with
`connection_ref: "http_cred:<name>"`, and the header is injected after any
summary of the call has been taken, so a secret cannot reach a log or an approval
prompt.

## Building it

The feature is behind a default-on `workflows` cargo feature in both crates, so a
slim build can drop the engine and its jq stack:

```sh
cargo build --no-default-features   # no workflows
```
