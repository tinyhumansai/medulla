# Workspace profiles (`MEDULLA.md`)

A `MEDULLA.md` at a repository root tells the orchestrator what that directory
**is** and how to route work over it. It is short by design: the orchestrator
reads it on every cycle, so it carries a ~100-200 token summary rather than the
full contents of `AGENTS.md`.

```markdown
---
harnesses: [claude-code, opencode]
models:
  reasoning: [claude-opus-4-8]
routing: |
  Billing changes -> the payments agent.
  Schema migrations -> review before delegating.
layout: |
  src/
  src/billing.rs
  src/stripe/
  migrations/
  README.md
---

Payments service. Owns billing, invoices, and the Stripe integration.
Decompose billing changes per bounded context and keep migrations in their
own task.
```

The frontmatter preferences are **advisory**. medulla renders them into the
orchestrator's context as guidance; it never gates delegation or model selection
on them. Everything is optional, and a profile that is only prose is valid.

`layout` is scanned from the directory rather than drafted: the summary says what
the workspace *is*, the layout says which paths it is made of, so the
orchestrator can decompose work at a file granularity instead of guessing at
entry points. It is bounded (two levels deep, ~40 entries) and skips build
output, dependency caches, and dotfiles.

## `medulla workspace`

```
medulla workspace add [dir] [--harness <id>] [--force] [--offline] [--config <path>]
medulla workspace list [--json] [--config <path>]
medulla workspace remove <dir|id> [--config <path>]
```

`add` is the command you want when setting up a new repository: it writes the
profile **and** registers the directory, so the orchestrator can actually see it
and place work there. Registration writes two config lists:

| List | What it does |
| --- | --- |
| `[workflow].workspaces` | Roots whose `MEDULLA.md` rides every backend session mint. Without an entry here the profile is never read at runtime. |
| `[fleet].workspaces` | The declared `Host → Harness → Workspace` chain the orchestrator places work onto. Without an entry here there is nowhere to *put* the work. |

Both are written to a single file: the explicit `--config` path, else the
highest-precedence file in the layered load, else `<medulla home>/config.toml`.
That is the same file the TUI writes, so the CLI and the running app agree on one
registry.

`add` is idempotent, and safe on a directory that already has a profile: an
existing `MEDULLA.md` is kept as it is and the directory is still registered, so
`medulla init` followed by `medulla workspace add` works. Re-running keeps the
entry's id and every hand-tuned field (name, harness, templates). Pass `--force`
to redraft the `MEDULLA.md` as well.

If the workspace's harness is not already declared, `add` declares it and its
host too. A workspace whose `harnessId` names nothing resolves to no harness and
no host, which cannot form a placement chain, so registering one without
completing it would not deliver what the command promises.

The registry is written as TOML. `--config` pointed at a `.json` file is refused
before anything is written, rather than leaving TOML at a path the next load
rejects.

`remove` takes a path or a registry id and only unregisters: the directory and
its `MEDULLA.md` are left alone.

## `medulla init`

```
medulla init [dir] [--force] [--offline] [--config <path>]
```

Drafts a profile for `dir` (default: the current directory) and writes
`MEDULLA.md` there:

1. Reads the directory's `AGENTS.md`, `CLAUDE.md`, and `README.md` (whichever
   exist) — recorded as `sources`, but not otherwise used.
2. Scans the file layout.
3. Writes a deterministic stub body alongside the scanned layout, for you to
   review and edit.

The model-drafted body went out with the memory layer that owned the provider
seam, so this stub is the only behaviour `init` has: `--offline` is accepted
but is now a no-op, since there is no model call left to skip. `--config
<path>` is likewise accepted but unused — `init` neither reads backend
settings nor writes the registry.

The layout is the part of the profile that carries real information, since it
is read straight off the tree rather than drafted. The summary is a starting
point for you to fill in by hand.

`init` authors the file and stops there; it does **not** register the workspace.
Use `medulla workspace add` for both.

The scaffold `init` fills in lives at `src/sdk/src/init/MEDULLA.md.tmpl`. It sits
inside the crate (rather than here under `docs/`) because it is embedded with
`include_str!` and the release image only copies `src/` and `vendor/`, so a
template outside the crate root fails that build.

### Flags

| Flag | Effect |
| --- | --- |
| `--force`, `-f` | Overwrite an existing `MEDULLA.md`. Without it, `init` refuses rather than discarding an authored profile. |
| `--offline` | Accepted for backward compatibility; a no-op, since `init` never calls a model. |
| `--config <path>` | Accepted but currently unused by `init`. |

## How a profile reaches the orchestrator

The profile is sent verbatim on the run request (`options.workspaceProfiles`,
`{ workspace, medullaMd }`); the backend parses it with the medulla SDK and folds
the result into the cycle. The orchestrator and reasoning tiers get the summary
and routing preferences appended to their system prompt, and an agent whose
workspace matches the profile's path gains a `profile:` line in `agent_list`.

Because the text crosses the wire unparsed, the format is owned by the SDK: a
format change ships with a library upgrade rather than a client release.

The `workspace` path must match what the roster reports for an agent
(`metadata.workspace`) for the profile to be attributed to that agent.
