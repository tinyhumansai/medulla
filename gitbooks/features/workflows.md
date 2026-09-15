---
description: >-
  A workflow is a saved multi-step plan whose agent steps each run as a real
  coding-agent session, in the order and with the parallelism the graph declares.
---

# Workflows

A workflow is a saved plan: a directed acyclic graph whose `agent` steps each open a real harness session (Claude Code, Codex, or OpenCode) in a workspace, with parallel branches and approval gates where a person has to say yes. Plenty of tools chain model calls. Here a step is a harness, with your credentials, in a directory, doing real work, and the graph only decides what runs when and what each step is given.

Workflows are optional. Medulla is a terminal first, and a build without the `workflows` feature drops the tab and the CLI verbs.

## Where they live

Workflows are JSON documents, one graph per file, in two layered directories, lowest precedence first:

```
<medulla home>/workflows/*.json     # yours, on this machine
<cwd>/.medulla/workflows/*.json     # this repository's, checked in
```

A workflow committed to a repository shadows a personal one with the same id. A malformed document costs only itself: the rest of the catalogue still loads and the failure is reported.

Run records live under `<medulla home>/state/workflows/runs/`, and the engine's checkpoints, which let a paused run survive a restart, under `state/workflows/checkpoints/`.

## In the TUI

Workflows is a top-level tab with three parts. The sidebar lists the installed workflows, with the selected one's runs indented beneath it; `↑↓` walk it, `1`-`9` jump, `Enter` opens the graph, `Esc` comes back. The canvas draws the graph, a box per node laid out left to right by distance from the trigger, a lane per concurrent branch; `←→` follows edges, `↑↓` walks lanes, `i` expands the selected node. Selecting a run overlays it, with each box recoloured by how that run left it and durations and diagnostics in the inspector.

The copilot (`c`) is a conversation that edits the graph. Ask for a change in plain words; a real harness session makes it, and the graph is re-read from disk afterwards so the transcript reports what changed rather than what the agent said it did.

`x` runs the selected workflow and `d` simulates it. `r` re-reads the store. A run's agent steps appear in the Sessions rail like any other session, and `Enter` on a session row that belongs to a run opens that run.

## From the command line

Every verb prints JSON and reads bulk input from stdin, so a person and an agent use the same surface:

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

`dry-run` is the one to reach for while authoring. Validation catches a malformed graph; a dry run catches a well-formed graph that is wired wrong, resolving every expression and checking every declared output shape with nothing dispatched.

`cancel` is process-local. A run started by `medulla workflow run` in one shell cannot be cancelled from another, and the command says so. The TUI can always cancel the run it started.

## Approval gates

A node with `config.requires_approval: true` parks the run. Release it with `medulla workflow resume <run-id> --approve <node-id>`, or refuse it with `--reject <node-id>`; both flags repeat. That is how a plan that ends in something irreversible gets a person in the middle without someone sitting and watching.

## Agents run and author them

Every harness Medulla launches is handed an MCP server, `medulla mcp`, with the workflow tools: the catalogue, `workflow_create`, `workflow_apply_ops` and `workflow_preview_ops`, `workflow_validate`, `workflow_dry_run`, `workflow_run`, `workflow_run_get`, `workflow_run_detail`, and `workflow_run_cancel`. So an agent in a session can start a saved workflow, or write a new one, without leaving its own transcript.

`workflow_run` answers with the run id rather than waiting, because a real workflow outlives any client's idle ceiling; `workflow_run_get` says how far it has got. `workflow_apply_ops` is the one that matters for editing: a patch is checked op by op, and a batch that fails anywhere leaves the workflow untouched.

`medulla skills install` goes the other way. It writes harness-native skill files (Claude Code skills, Codex prompts, or a generic form) that trigger your saved workflows, into your home, the current checkout, or Medulla's own managed root, so an everyday session started outside Medulla can still find and start one. See [CLI Reference › `medulla skills`](../developers/cli-reference.md#medulla-skills).

## Evolve

`medulla workflow evolve <id>` asks a harness to review a workflow's own run history and file notes and proposals against it. Nothing is applied on its own: `medulla workflow proposals`, `accept`, and `reject` are how a person decides. `[workflows.evolve] autoOnFailure = true` queues a review after a failed run.

## Read next

The full authoring reference, covering the node-kind catalogue, `agent` node config, and jq expressions, is in [`docs/workflows.md`](https://github.com/tinyhumansai/medulla/blob/main/docs/workflows.md) in the repository.
