---
description: >-
  A short authored file at a repository root that tells the orchestrator what the
  directory is and how to route work over it.
---

# MEDULLA.md Workspace Profiles

An orchestrator's roster tells it *who* it can delegate to. It says nothing about
*what a directory is*. Handed five repositories and a fleet of harnesses, a model
with no other information will guess — and guess plausibly, and guess wrong.

`AGENTS.md` and `CLAUDE.md` don't solve this. They are written for a coding agent
working *inside* a repository: long, prose-heavy, and carrying no routing
preferences at all. An orchestrator reads its context on every cycle, so a
thousand-line house-style document is exactly the wrong shape.

`MEDULLA.md` is the right shape. One short file at a workspace root, ~100–200
tokens, describing what the repository is and how work over it should be routed.

## What one looks like

YAML frontmatter for machine-readable preferences, Markdown body for the summary:

```markdown
---
harnesses: [claude-code, opencode]
models:
  reasoning: [claude-opus-4-8]
  compress: [claude-haiku-4-5]
routing: |
  Billing changes -> the payments agent.
  Schema migrations -> review before delegating.
  Never delegate work that touches credentials.
---

Payments service. Owns billing, invoices, and the Stripe integration.
Decompose billing changes per bounded context and keep migrations in their
own task.
```

Every key is optional, and **the body is the load-bearing part**. A profile that
is nothing but prose is completely valid. The frontmatter is a bonus.

| Key | What it expresses |
| --- | --- |
| `harnesses` | Harnesses to prefer for work in this workspace. |
| `models` | Preferred models, optionally scoped per tier (`reasoning`, `compress`, `orchestrator`). A flat list reads as a reasoning-tier preference. |
| `routing` | Freeform guidance — the rules that don't fit a schema. |

Write the body as instructions for an orchestrator, not marketing copy: what the
code does, the important entry points, and the house rules that should shape how
work gets decomposed. The first line becomes the repository's one-sentence
identity in the roster, so lead with it.

## Advisory, never enforced

This is the design decision that matters most. Medulla renders your preferences
into the orchestrator's context as **guidance the model reasons over**. It does
not filter delegation targets, reject models, or fail a cycle over a violation.

Routing is a cognitive decision. A hard constraint — this model may never be
used, this directory is off limits — is policy, and policy belongs to the host
system that can actually enforce it, not to a document the model reads.

The same instinct runs through the format: parsing never throws. Malformed
frontmatter degrades to a summary-only profile rather than failing. A profile is
operator-authored, and an authored file should never be able to break a running
operation.

## One per root, no cascade

`CLAUDE.md` and `AGENTS.md` cascade — a file in a subdirectory layers over one
above it. `MEDULLA.md` deliberately does not. There is no global-versus-project
precedence and no merge semantics: every profile in play is presented together,
each tagged with the workspace it describes.

For an umbrella repository with several component repositories underneath, that
means each component root carries its own profile, and the orchestrator sees all
of them side by side rather than a resolved winner.

## What it changes

A profile shapes two things:

* **The orchestrator's and reasoning tier's context**, which is where routing
  preferences and the summary land — appended below whatever prompt is already
  in play, never replacing it.
* **The roster**, where an agent rooted in a profiled workspace gains a
  one-line identity drawn from the summary.

Workers themselves never read `MEDULLA.md`. It shapes how work is decomposed and
who it goes to, upstream of any worker starting. With no profiles anywhere,
behaviour is identical to a build that never had the feature.

## Writing one

```sh
medulla init            # draft a MEDULLA.md for the current directory
medulla init ./payments # or for a specific one
```

`init` reads whatever the directory already has — `AGENTS.md`, `CLAUDE.md`,
`README.md` — and asks a model to distil it into a summary plus routing hints.
That draft is a starting point, not the final word: the summary is what the
orchestrator actually reads on every cycle, so it is worth a pass by hand.

`--force` overwrites an existing profile (without it, `init` refuses rather than
discarding authored work). `--offline` skips the model and writes an editable
stub. If no model is available, or the call fails, `init` degrades to the stub
and says so — it always leaves you a usable file.

See [CLI Reference](../developers/cli-reference.md) for the full flag list and
[Orchestrator Routing](routing.md) for how these preferences meet Medulla's own
routing decisions.
