---
description: >-
  A task is one instruction handed to one harness. A workflow is a saved
  multi-step plan whose steps each run as a real coding-harness session.
---

# Workflows

A Medulla task is one instruction handed to one harness. A **workflow** is a
saved, multi-step plan: a directed acyclic graph whose `agent` steps each run as
a real coding-harness session — Claude Code, Codex, or OpenCode — in the order
and with the parallelism the graph declares.

That is the distinction worth holding on to. Plenty of tools let you chain model
calls. Here an `agent` node is a *dispatched task*: a harness, with your
credentials, in a workspace, doing real work. The graph decides what runs when,
what each step is given, and where a human has to say yes.

## Where they live

Workflows are JSON documents, one graph per file, in two layered directories —
lowest precedence first:

```
<medulla home>/workflows/*.json     # yours, on this machine
<cwd>/.medulla/workflows/*.json     # this repository's, checked in
```

A workflow committed to a repository shadows a personal one with the same id, the
same way `.medulla/agents` layers. A malformed document costs only itself: the
rest of the catalogue still loads and the failure is reported rather than
swallowed.

Run records live under `<medulla home>/state/workflows/runs/`, and the engine's
checkpoints — what lets a paused run survive a restart — under
`state/workflows/checkpoints/`.

## In the TUI

Workflows is a top-level tab with three parts:

* **The sidebar** lists the installed workflows, with the selected one's runs
  indented beneath it. `↑↓` walk it, `1`-`9` jump, `Enter` opens the graph, `Esc`
  comes back.
* **The canvas** draws the graph: a box per node, laid out left to right by
  distance from the trigger, a lane per concurrent branch, and each branch's port
  name written on the wire that carries it. `←→` follows edges, `↑↓` walks the
  lanes, and `i` expands the selected node's whole declaration. Selecting a run
  overlays it — each box recoloured by how that run left it, unreached steps
  dimmed, and durations and diagnostics in the inspector.
* **The copilot** (`c`) is a conversation that edits the graph. Ask for a change
  in plain words; a real harness session makes it, and the graph is re-read from
  the store afterwards so the transcript reports what actually changed rather
  than whatever the agent said it did.

`x` runs the selected workflow and `d` simulates it, from either pane. `r`
re-reads the store.

The tab is present when the crate is built with its default `workflows` feature.
A slim build without the engine drops the tab rather than offering one that
cannot draw anything.

## From the command line

Every verb prints JSON and reads bulk input from stdin, so the same surface is
usable by a person and by an agent without either being a special case:

```sh
medulla workflow list                  # what is installed
medulla workflow get <id>              # one workflow, whole
medulla workflow create <id>           # install from a document on stdin
medulla workflow apply-ops <id>        # edit by graph patch, from stdin
medulla workflow validate [id]         # check a saved workflow, or stdin
medulla workflow catalog [kind]        # the node kinds an author may use
medulla workflow dry-run <id>          # simulate: no harness, no network
medulla workflow run <id>              # for real, on this machine's CLIs
medulla workflow list-runs <id>        # history
medulla workflow resume <run-id> --approve <node-id>
medulla workflow cancel <run-id>
```

`dry-run` is the one to reach for while authoring. Validation catches a malformed
graph; a dry run catches a *well-formed* graph that is wired wrong — every
expression resolved and every declared output shape satisfied, against capability
stand-ins, with nothing dispatched.

`cancel` is process-local. A run started by `medulla workflow run` in one shell
cannot be cancelled from another, because there is no control channel between two
CLI invocations, and the command says so rather than reporting a bare failure.
The paths that can always cancel are the ones that own the running process: the
TUI cancels the run it started, and an orchestrator's abort frame reaches the
daemon executing it.

## Approval gates

A node with `config.requires_approval: true` parks the run instead of continuing.
Release it with `medulla workflow resume <run-id> --approve <node-id>`, or refuse
it with `--reject <node-id>`; both flags repeat. This is how a plan that ends in
something irreversible gets a human in the middle of it without a person having
to sit and watch the run.

## Agents author them too

Medulla drives Claude Code and Codex over [ACP](https://github.com/tinyhumansai/medulla/blob/main/docs/acp-harnesses.md)
as a *client*, so it cannot hand them tools directly — it offers them. Every ACP
session gets an MCP server (`medulla workflow mcp`, the same binary) exposing the
catalogue, `workflow_create`, `workflow_apply_ops` / `workflow_preview_ops`,
`workflow_validate`, `workflow_dry_run`, and `workflow_runs`.

`workflow_apply_ops` is the one that matters for editing. Rewriting a whole
document loses whatever the model misremembered; a patch is checked op by op, and
a batch that fails anywhere leaves the workflow untouched.

## Read next

The full authoring reference — the node-kind catalogue, `agent` node config, jq
expressions, and how the orchestrator dispatches a workflow to a worker — is in
[`docs/workflows.md`](https://github.com/tinyhumansai/medulla/blob/main/docs/workflows.md)
in the repository.
