---
description: >-
  A local, operator-owned task ledger and the external sources that feed it.
  Separate from the orchestrator's internal ledger and from a repository's
  MEDULLA.md.
---

# Tasks and Sources

Medulla keeps a plain task list you own, on disk, independent of whichever
runtime is driving your chat. It is not the orchestrator's internal task ledger —
that is the record of a live fan-out, covered in
[Token Efficiency and Budgets](token-efficiency.md). This is the human-facing
inventory of work: the things you want done, plus the external sources that
suggest new ones.

The store lives at `<medulla-home>/tasks.json`, typically
`~/.medulla/tasks.json`, and holds two things — the tasks themselves and the
configured sources. It is one of several independent stores Medulla keeps for
you, and none of them overwrites another:

| Store | Scope | Primary consumer |
| --- | --- | --- |
| `tasks.json` | User-level task inventory and sources | The Tasks tab |
| `MEDULLA.md` | Repository-level routing profile | [Orchestrator routing](routing.md) |
| `workflows/*.json` | Authored multi-step plans | [Workflows](workflows.md) |

## The Tasks tab

The Tasks tab has two pages, `All Tasks` and `Sources`. From `All Tasks` you can
load the document, create a task, rename it, edit its description, mark its
status, delete it, and save the whole document back. A task carries a stable ID,
a title and description, a status, creation and update timestamps, and — when it
came from a source — the source's identity and URL.

Status is one of four states: **open**, **in progress**, **done**, or
**cancelled**. A task can also carry a recurrence, and when a recurring
definition comes due Medulla can spin a concrete task off it rather than making
you retype the same work each week.

The document is written carefully because it is yours to hand-edit. The
repository takes an exclusive lock across the whole read-modify-save so two
writers cannot race, writes pretty-printed JSON through a temporary file and an
atomic rename so a crash mid-write cannot corrupt it, and treats a malformed file
as an error rather than silently starting over. A missing file simply starts
empty.

## GitHub sources

A source pulls work in from outside. Today that means GitHub: from the `Sources`
page you add one by `owner/repository`, and a sync pulls its issues into the
ledger. A source configuration names the owner and repository plus the issue
state, labels, an optional filter, and an optional token, and each sync reports
how many items it added, updated, left unchanged, and errored on.

Two properties keep sync from trampling your edits. Synchronized records are
merged in, but your local title, description, and status edits are preserved — a
sync updates a task without discarding what you changed about it. And a provider
error comes back in the sync result instead of silently dropping the source, so a
rate limit or a bad token is visible rather than mysterious. A source token is a
configuration secret and is handled as one; it is not rendered casually into the
UI.

## Where it sits

The task ledger is deliberately small and deliberately local. It does not try to
be a project tracker, and it does not reach into your repositories the way
`MEDULLA.md` describes them. It is the short list of what you mean to get done,
kept somewhere the orchestrator can see it and somewhere you can edit it by hand.

See [CLI Reference](../developers/cli-reference.md) for the commands that touch
these stores and [MEDULLA.md Workspace Profiles](workspace-profiles.md) for the
per-repository profile it sits beside.
