# Plan: harness skills that trigger Medulla workflows over MCP

## Problem

A workflow is only reachable today from Medulla itself — the TUI, `medulla workflow run`,
or a harness that Medulla spawned and handed the `workflow_*` MCP family to. An operator
sitting in their *own* Claude Code or Codex session has no way to say "babysit this PR"
and have the saved `babysit` workflow start.

The missing piece is not transport. `medulla mcp` already serves `workflow_run` to any MCP
client. What is missing is *discovery and phrasing*: nothing tells a harness that a
workflow named `babysit` exists, what it takes, or that the way to start it is a tool call
with a particular argument shape. That is exactly what a skill is for.

So: generate, per saved workflow, a harness-native skill whose body instructs the model to
call `mcp__medulla__workflow_run` with the workflow's declared inputs.

## What was verified before writing this

Facts this plan rests on, each checked against the tree at `11351b34`:

- **`medulla mcp` standalone works with no grant.** Driving `./target/debug/medulla mcp`
  with `initialize` + `tools/list` over stdio returns `protocolVersion 2024-11-05` and 19
  tools: `workflow_list … workflow_propose`, including `workflow_run`. No `fleet_*` — a
  process with no hub mints no grant (`src/sdk/src/mcp/mod.rs`).
- **Declared inputs are already on the listing view.** `WorkflowSummary.inputs`
  (`src/sdk/src/workflows/types/workflow.rs:176`) carries `Vec<WorkflowInput>` —
  `name`, `type`, `description`, `required`, `default`
  (`vendor/openhuman/vendor/tinyflows/src/model/inputs.rs:112`). Rendering a skill needs
  only `workflow_list`, not a graph fetch.
- **Tool-surface gating already exists.** `ToolMode::{Full,Propose}` +
  `MEDULLA_WORKFLOW_TOOLS` (`src/sdk/src/workflows/mcp/evolve.rs`) withhold verbs from
  `tools/list` *and* `tools/call`, failing closed on an unknown value. A third mode is a
  small, idiomatic addition rather than a new mechanism.
- **`workflow_run` blocks for the whole run.** `dispatch.rs:51-53` says runs "may take
  minutes" and are deliberately left concurrent; the call returns the finished run record.
  This is the one real design constraint (see *Long runs*).
- **Skill directories on this machine.** `~/.claude/skills/<name>/SKILL.md` and
  `~/.codex/skills/<name>/SKILL.md` (codex-cli 0.146.0) both exist and hold the same
  frontmatter shape (`name`, `description`, optional `allowed-tools`). `~/.cursor/skills-cursor`
  exists; its contract is *not* verified and is out of scope for phase 1.

## Design

### 1. One renderer, several targets

New SDK module `src/sdk/src/workflows/skills/`:

| file | responsibility |
| --- | --- |
| `mod.rs` | module docs, wiring, `pub use` |
| `types.rs` | `SkillTarget`, `SkillScope`, `RenderedSkill`, `InstallPlan`, `InstallOutcome` |
| `render.rs` | `WorkflowSummary` → `RenderedSkill` (frontmatter + body) |
| `targets.rs` | target → filesystem layout, scope resolution, slugging |
| `install.rs` | plan/apply/remove, marker discipline, drift detection |
| `tests.rs` | unit tests |

Rendering is target-independent: one `RenderedSkill { slug, description, body }`, and each
target decides where it lands and whether it also emits a slash-command variant. That keeps
harness churn (and there will be churn — Codex's skill support is young) confined to
`targets.rs`.

**Skill body outline** (generated, not hand-written per workflow):

```markdown
---
name: medulla-babysit
description: >
  <workflow description>. Use when the operator asks to run the Medulla
  "babysit" workflow, or describes the work it does.
---

# Run the `babysit` Medulla workflow

Start it with the Medulla MCP server:

  mcp__medulla__workflow_run
  { "id": "babysit", "inputs": { "pr": "<number>", "repo": "<owner/name>" } }

## Inputs
| name | type | required | meaning |
| pr   | string | yes | The pull request to babysit. |
| repo | string | no (default: current) | … |

Collect a required input from the operator before calling; do not invent one —
a missing or misnamed input is rejected and nothing runs.

## Reading the result
The call returns the whole run record … report the failing step …

## If the tool is not available
The Medulla MCP server is not attached to this session. Fall back to
`medulla workflow run babysit --inputs '{"pr":"123"}'`, or attach the server
with `medulla skills install --with-mcp`.
```

The fallback paragraph matters: skills are copied into user-scope directories that outlive
any particular MCP configuration, and a skill that dead-ends when the server is missing is
worse than no skill.

### 2. Target layouts

| target | skill | slash command |
| --- | --- | --- |
| `claude` | `<root>/.claude/skills/medulla-<id>/SKILL.md` | `<root>/.claude/commands/medulla-<id>.md` (`argument-hint`, `$ARGUMENTS`) |
| `codex` | `<root>/.codex/skills/medulla-<id>/SKILL.md` | `<root>/.codex/prompts/medulla-<id>.md` |
| `generic` | `<root>/.medulla/skills/medulla-<id>/SKILL.md` | managed block appended to `AGENTS.md` |

`<root>` is `$HOME` for `--scope user` and the project directory for `--scope project`
(where the project layout is `.claude/…` at the repo root). Slash commands are opt-in
(`--with-commands`) — a skill already gives the model the trigger; the command is for the
operator who wants to type it.

### 3. Managed-file discipline

Every generated file opens with a marker line:

```
<!-- medulla:managed workflow=babysit rev=<sha256 of rendered body> -->
```

- `install` writes only files that do not exist, or that exist **and carry the marker**.
  An unmarked collision is reported and skipped, never overwritten.
- `sync` rewrites marked files whose `rev` differs, and deletes marked files whose workflow
  no longer exists or is `enabled: false`.
- `uninstall` removes only marked files.

This is the whole safety story for touching `~/.claude`, and it is worth unit-testing more
carefully than the rendering.

### 4. A run-only tool mode

Add `ToolMode::Run` beside `Full`/`Propose`, selected by `MEDULLA_WORKFLOW_TOOLS=run`,
serving only `workflow_list`, `workflow_get`, `workflow_dry_run`, `workflow_run`,
`workflow_runs`, `workflow_run_get`. Everything else is withheld — the same
absent-from-`tools/list` enforcement `Propose` already uses.

Rationale: a skill installed into the operator's everyday harness gives that harness the
authoring surface too, so an unrelated turn could rewrite or delete a graph. Trigger-only
sessions should not have the verbs. `medulla skills install --with-mcp` writes
`MEDULLA_WORKFLOW_TOOLS=run` into the server's env by default; `--tools full` opts back in.

`from_wire` keeps failing closed to the most restricted mode.

### 5. MCP registration

A skill without the server attached is inert, so `install --with-mcp` also registers it:

- **Claude, project scope** — merge `mcpServers.medulla` into `<project>/.mcp.json`.
- **Claude, user scope** — shell out to `claude mcp add --scope user medulla -- <medulla> mcp`
  when the CLI is on `PATH`; otherwise print the exact command.
- **Codex** — merge `[mcp_servers.medulla]` into `~/.codex/config.toml`
  (`command`, `args = ["mcp"]`, `env`).

`command` is the resolved absolute path of the running binary
(`std::env::current_exe`), not the bare name — a user-scope entry has no `PATH` guarantees.
Run `medulla::mcp::preflight` first and refuse with its message rather than writing a
registration that cannot serve tools.

### 6. CLI

```
medulla skills list   [--json]
medulla skills install [<workflow-id>…] [--harness claude,codex,generic|all]
                       [--scope user|project] [--dir <path>]
                       [--with-mcp] [--with-commands] [--tools run|full]
                       [--dry-run] [--json]
medulla skills sync    [same target flags] [--prune]
medulla skills uninstall [<workflow-id>…] [same target flags]
```

Parsed in `src/tui/src/cli/parse.rs` (new `Command::Skills`), implemented in
`src/tui/src/commands/skills.rs`, all logic in the SDK so the future MCP verb and the TUI
action reuse it. Follows the existing `workflow` command contract: JSON on stdout, errors
on stderr, non-zero exit.

No default harness: with none named, install to every target whose directory already
exists, and say which in the output.

### 7. Keeping skills current

Three ways in, all calling the same `sync`:

1. `medulla skills sync` by hand.
2. Config opt-in `[workflow.skills] autosync = true`, plus `targets`, `scope`, and
   `include`/`exclude` id lists; honoured after `workflow_create` / `apply_ops` / `delete`
   in `ops`. Off by default — writing into `~/.claude` as a side effect of an authoring
   call should be something the operator asked for once, explicitly.
3. A TUI action on the Workflows screen ("Install as skill"), phase 3.

### Long runs

`workflow_run` returns only when the run finishes. For a `babysit`-class workflow that is
an hour-long tool call: most MCP clients will time out or the operator will interrupt, and
the run's outcome is then unobservable from the harness even though it is still going.

- **Phase 1** ships against the blocking verb and says so in the generated body ("this call
  blocks until the run finishes; it can take minutes"). Honest, and adequate for short
  workflows.
- **Phase 2** adds `workflow_start` → `{ runId }` returning as soon as the run is admitted,
  with the run driven to completion in the background, and the generated body becomes
  start → report the id → poll `workflow_run_get`. This is the version a babysit workflow
  actually needs. The detached executor is the real work here (ownership of the run future
  when the MCP process is short-lived, and whether it should instead be handed to a running
  daemon over the control socket, which today has no workflow-run op — only
  `WorkflowAdvert` on the worker side).

Phase 2 is not a stretch goal to be quietly dropped; it is the difference between the
feature working for the motivating example and not. Phase 1 exists so the skill plumbing
can be verified independently of it.

## Delivery order

1. **Landed.** `skills` module: types, render, targets, marker discipline, unit tests. No
   I/O beyond a tempdir in tests.
2. **Landed.** `medulla skills` CLI (`install`/`list`/`uninstall`/`sync`, `--dry-run`).
3. **Landed.** `ToolMode::Run` + `--with-mcp` registration.
4. **Landed.** Docs: `docs/workflows.md` section, `src/sdk/src/workflows/skills/README.md`.
5. Phase 2 — `workflow_start` + polling, and regenerate bodies to use it.
6. Phase 3 — TUI action, `autosync` config.

Steps 1–4 are one PR; 5 and 6 are separate.

### What steps 1–4 did differently

The plan above is left as written; this is what the implementation chose instead, and why.

- **Registration never shells out.** §5 said Claude user scope would run
  `claude mcp add --scope user …` when the CLI is on `PATH`. It does not. Every writable
  target is a config merge we perform ourselves (`.mcp.json`, `~/.codex/config.toml`),
  because a subprocess makes the outcome depend on which CLI version is on `PATH`, cannot
  be tested offline, and cannot honour `--dry-run`. Claude user scope and `generic` return
  a `manual` outcome carrying the exact command for the operator to run. This is a new
  file, `registration.rs` (plus `registration_tests.rs`), not in the §1 table.
  `mcp::preflight` runs in the CLI before anything is written, as §5 required.
- **`generic` has no slash-command variant.** §2 proposed a managed block appended to
  `AGENTS.md`. Dropped: `AGENTS.md` is a hand-edited file with no marker convention of its
  own, and appending to it is exactly the kind of write the marker discipline exists to
  avoid. `generic` gets a skill file under `.medulla/skills/` and nothing else.
- **Type names.** §1 named `InstallPlan` / `InstallOutcome`. The landed shapes are
  `InstallOptions` (input), `InstallReport` (a `Vec<FileOutcome>`), `FileAction`
  (`created`/`updated`/`unchanged`/`skippedUnmanaged`/`removed`), and `InstalledSkill` for
  the listing. `--dry-run` is a field on `InstallOptions` rather than a separate plan type:
  one code path reaching identical decisions is what makes a dry run trustworthy.
- **Unknown `MEDULLA_WORKFLOW_TOOLS` resolves to `Propose`, not `Run`.** §4 said "the most
  restricted mode", which is ambiguous now there are three. `Propose` is the one that can
  neither write a graph nor start a run; a typo resolving to `Run` would hand an
  unrecognised caller the ability to execute harness sessions and `code` nodes for real.
- **Test layout.** Unit coverage lives in `src/sdk/src/workflows/skills/tests.rs`
  (rendering, target layout, idempotence, collisions, prune, dry run),
  `registration_tests.rs` (merge, preservation, manual and skipped outcomes),
  `src/sdk/src/mcp/tests/run_mode.rs` (the allow-list from both directions, plus the
  `tools/call` refusal), and `src/tui/src/cli/tests/skills.rs` (flag parsing). The two
  feature suites *Verification* asks for exist —
  `src/sdk/tests/feature_workflow_skills.rs` drives `ops::create` → `FileWorkflowStore` →
  `store.list()` → `skills::install` over a tempdir, and
  `src/sdk/tests/feature_workflow_run_mode.rs` covers the trigger-only surface. The
  latter calls `handle_request` in-process rather than spawning `medulla mcp`: the
  spawning variant buys only the stdio framing, which `tests/e2e_cli.rs` already covers,
  at the cost of a test that depends on a built binary. Driving the real process in
  `MEDULLA_WORKFLOW_TOOLS=run` is therefore a manual step (below), not an automated one.
- **CLI aliases.** `skills` and `skill` both reach the command, verbs accept `add`,
  `refresh`, `remove`/`rm`, and a bare `medulla skills` lists. Unrecognised flags are
  tolerated the way the other parsers tolerate them.
- **The catalogue-skill prototype was not built.** *Risks* suggested prototyping a single
  `medulla-workflows` skill beside the per-workflow form and choosing between them. Only
  the per-workflow form exists; if sprawl bites, that alternative is still open.
- **Manual verification is partly done.** Steps 1 and 2 of the manual list are recorded:
  a `babysit` workflow seeded through `medulla workflow create` into a tempdir
  `MEDULLA_HOME`, `skills install babysit --harness claude,codex --dir $H --with-mcp
  --json` writing both `SKILL.md` files and `[mcp_servers.medulla]` in
  `.codex/config.toml`, a second identical run reporting `unchanged` for every file and
  the registration, an unmarked neighbour surviving install and `sync --prune`, and
  `uninstall` removing only the marked files and their now-empty directories. The
  trigger-only surface was also driven by hand: `MEDULLA_WORKFLOW_TOOLS=run medulla mcp`
  over stdio answers `tools/list` with exactly the six read/run verbs and refuses
  `tools/call workflow_delete` with the explanatory message, leaving the workflow on
  disk.

  Steps 3–6 still need an operator. In particular step 3 — a live `claude --print` /
  `codex exec` run selecting the skill and calling `mcp__medulla__workflow_run` — is the
  one that can fail in an interesting way, and nothing here substitutes for it.

## Verification

**Automated** (all offline, per the repo's testing rules):

- `src/sdk/src/workflows/skills/tests.rs`
  - slug and frontmatter rendering for a workflow with zero, optional-only, and required
    inputs; description non-empty even when the workflow's is;
  - `rev` changes iff the body changes;
  - target paths for each harness × scope;
  - install is idempotent — second run writes nothing and reports `unchanged`;
  - an unmarked file at the target path is skipped, exit reports the collision, file bytes
    unchanged;
  - `sync --prune` deletes the marked skill of a deleted workflow and leaves an unmarked
    neighbour alone;
  - a disabled workflow gets no skill.
- `src/sdk/src/workflows/mcp/` tests: `ToolMode::Run` serves exactly the six read/run verbs;
  `tools/call` on a withheld verb is refused with the explanatory message; an unknown
  `MEDULLA_WORKFLOW_TOOLS` value resolves to the most restricted mode.
- `src/tui/src/cli/tests.rs`: parse coverage for every flag, including repeated
  `--harness a,b` and `--scope`.
- `src/sdk/tests/feature_workflow_skills.rs`: end-to-end over a tempdir `MEDULLA_HOME` —
  create two workflows through `ops`, install to a fake `$HOME`, assert the files, edit one
  workflow, `sync`, assert one file changed and one did not.
- `src/sdk/tests/` MCP e2e: spawn `medulla mcp` with `MEDULLA_WORKFLOW_TOOLS=run` against a
  seeded store, assert the reduced `tools/list`, then `workflow_run` a trivial mock-harness
  workflow and assert a run record comes back.
- Coverage stays above the 80% line gate; `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean.

**Manual, and required before calling it done** — the automated tests prove we wrote files,
not that a harness reads them:

1. Scratch home: `export H=$(mktemp -d)`, seed a `babysit` workflow in
   `MEDULLA_HOME=$H/.medulla`, then
   `HOME=$H medulla skills install babysit --harness claude,codex --scope user --with-mcp`.
2. Inspect the tree; re-run and confirm `unchanged`.
3. `HOME=$H claude --print "run the babysit workflow for PR 123"` — confirm from the
   transcript that the skill was selected and `mcp__medulla__workflow_run` was called with
   `id: "babysit"` and the input. Repeat with `codex exec` against `~/.codex/skills`.
4. `HOME=$H claude mcp list` shows `medulla` connected.
5. Negative: delete the MCP registration, re-run — the model should follow the documented
   CLI fallback rather than claiming it started a run.
6. TUI unchanged: `tmux new-session -d -s agent-medulla-$$ … MEDULLA_HOME=$H ./target/debug/medulla`,
   capture the Workflows pane.

Step 3 is the one that can actually fail in an interesting way (a description the model
does not match on), and it is why the description line is generated from the workflow's own
description plus an explicit "use when" clause rather than from the id alone.

## Risks

- **Harness formats drift.** Codex skills are new; Cursor is unverified and deliberately
  excluded. Confined to `targets.rs`, and the generic/`AGENTS.md` target is the escape hatch.
- **Writing into `~/.claude`.** Mitigated by the marker discipline, `--dry-run`, and
  autosync being opt-in.
- **Escalation.** A skill hands every session in that harness the ability to start real
  runs — that is the point, but `ToolMode::Run` is what keeps it from also handing over
  graph editing and deletion.
- **Skill sprawl.** Twenty workflows become twenty skills competing for the model's
  attention. If it bites, the answer is one `medulla-workflows` skill that lists the
  catalogue and calls `workflow_list` — worth prototyping alongside the per-workflow form
  in step 1 and choosing with the step 3 check.
