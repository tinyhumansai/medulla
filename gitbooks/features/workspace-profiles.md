---
description: >-
  A short authored file at a repository root that tells the orchestrator what the
  directory is and how to route work over it.
---

# MEDULLA.md Workspace Profiles

An orchestrator's roster tells it who it can delegate to. It says nothing about
what a directory actually is. Handed five repositories and a fleet of harnesses,
a model with no other information will guess, and it will guess plausibly and
wrongly.

`AGENTS.md` and `CLAUDE.md` do not solve this. They are written for a coding
agent working inside a repository, so they are long, prose-heavy, and carry no
routing preferences at all. An orchestrator reads its context on every cycle,
which makes a thousand-line house-style document the wrong shape.

`MEDULLA.md` is the right shape: one short file at a workspace root, roughly 100
to 200 tokens, describing what the repository is and how work over it should be
routed.

## What one looks like

YAML frontmatter carries the machine-readable preferences and the Markdown body
carries the summary.

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

Every key is optional, and the body is the load-bearing part. A profile that is
nothing but prose is completely valid. The frontmatter is a bonus.

| Key | What it expresses |
| --- | --- |
| `harnesses` | Harnesses to prefer for work in this workspace. |
| `models` | Preferred models, optionally scoped per tier (`reasoning`, `compress`, `orchestrator`). A flat list reads as a reasoning-tier preference. |
| `routing` | Freeform guidance, for the rules that do not fit a schema. |

Write the body as instructions for an orchestrator rather than marketing copy:
what the code does, the important entry points, and the house rules that should
shape how work gets decomposed. The first line becomes the repository's
one-sentence identity in the roster, so lead with it.

## Advisory, never enforced

This is the design decision with the most consequences. Medulla renders your
preferences into the orchestrator's context as guidance the model reasons over.
It does not filter delegation targets, reject models, or fail a cycle over a
violation.

Routing is a cognitive decision. A hard constraint, such as forbidding a model
outright or putting a directory off limits, is policy, and policy belongs to the
host system that can actually enforce it rather than to a document the model
reads.

The same instinct runs through the format. Parsing never throws, so malformed
frontmatter degrades to a summary-only profile instead of failing. A profile is
operator-authored, and an authored file should never be able to break a running
operation.

## One per root, no cascade

`CLAUDE.md` and `AGENTS.md` cascade, so a file in a subdirectory layers over one
above it. `MEDULLA.md` deliberately does not. There is no global-versus-project
precedence and no merge semantics. Every profile in play is presented together,
each tagged with the workspace it describes.

For an umbrella repository with several component repositories underneath, that
means each component root carries its own profile, and the orchestrator sees all
of them side by side instead of a resolved winner.

## What it changes

A profile shapes two things. It shapes the orchestrator's and reasoning tier's
context, which is where the routing preferences and the summary land, appended
below whatever prompt is already in play rather than replacing it. It also shapes
the roster, where an agent rooted in a profiled workspace gains a one-line
identity drawn from the summary.

Workers themselves never read `MEDULLA.md`. It shapes how work is decomposed and
who it goes to, upstream of any worker starting. With no profiles anywhere,
behaviour is identical to a build that never had the feature.

## Writing one

```sh
medulla init            # draft a MEDULLA.md for the current directory
medulla init ./payments # or for a specific one
```

The `init` command reads whatever the directory already has, meaning `AGENTS.md`,
`CLAUDE.md`, and `README.md`, then asks a model to distil it into a summary plus
routing hints. Treat that draft as a starting point rather than the final word.
The summary is what the orchestrator actually reads on every cycle, so it is
worth a pass by hand.

The `--force` flag overwrites an existing profile; without it, `init` refuses
rather than discarding authored work. The `--offline` flag skips the model and
writes an editable stub. If no model is available, or the call fails, `init`
degrades to the stub and says so, so it always leaves you a usable file.

See [CLI Reference](../developers/cli-reference.md) for the full flag list and
[Orchestrator Routing](routing.md) for how these preferences meet Medulla's own
routing decisions.
