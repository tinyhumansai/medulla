# Workspace profiles (`MEDULLA.md`)

A `MEDULLA.md` at a repository root tells the orchestrator what that directory
**is** and how to route work over it. It is short by design — the orchestrator
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
---

Payments service. Owns billing, invoices, and the Stripe integration.
Decompose billing changes per bounded context and keep migrations in their
own task.
```

The frontmatter preferences are **advisory**. medulla renders them into the
orchestrator's context as guidance; it never gates delegation or model selection
on them. Everything is optional — a profile that is only prose is valid.

## `medulla init`

```
medulla init [dir] [--force] [--offline] [--config <path>]
```

Drafts a profile for `dir` (default: the current directory) and writes
`MEDULLA.md` there:

1. Reads the directory's `AGENTS.md`, `CLAUDE.md`, and `README.md` (whichever
   exist).
2. Asks the configured model to distil them into a summary plus routing hints.
3. Writes the result for you to review and edit.

The draft is a starting point, not the final word — the summary is what the
orchestrator actually reads, so it is worth a pass by hand.

The scaffold `init` fills in lives at `src/sdk/src/init/MEDULLA.md.tmpl`. It sits
inside the crate (rather than here under `docs/`) because it is embedded with
`include_str!` and the release image only copies `src/` and `vendor/` — a
template outside the crate root fails that build.

**Flags**

| Flag | Effect |
| --- | --- |
| `--force`, `-f` | Overwrite an existing `MEDULLA.md`. Without it, `init` refuses rather than discarding an authored profile. |
| `--offline` | Skip the model call and write the editable stub. |
| `--config <path>` | Explicit config file for the backend/model settings. |

**Model resolution** matches `medulla memory ingest`: an explicit
`OPENROUTER_API_KEY` wins, otherwise the backend's inference surface is used with
the JWT from `medulla login`. With neither — or if the model call fails — `init`
writes the stub and says so, so it always leaves you a usable file.

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
