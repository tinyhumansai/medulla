# Skills

Harness-native skills that trigger saved workflows over MCP.

`medulla mcp` already serves `workflow_run` to any MCP client, so an operator's
own Claude Code or Codex session is one tool call away from starting a saved
workflow. Nothing in that session knows it. The missing piece was never
transport — it is discovery and phrasing: which workflows exist, what each one
takes, and that the way to start one is a tool call with a particular argument
shape. That is what a skill file is for, and this module generates one per
workflow.

## Rendering is target-independent

A workflow renders to exactly one body, and only [`targets.rs`](./targets.rs)
decides where that body lands. Codex's skill support is young and Cursor's is
unverified, so harness churn is guaranteed; confining it to path arithmetic
means a new harness is a match arm, not a second template to keep in step with
the first.

The generated body is built from `WorkflowSummary` alone — the declared inputs
are already on the listing view, so rendering never has to fetch a graph. It
contains four things that each exist for a reason:

- **A description with an explicit "use when" clause.** The whole feature turns
  on the model matching a paraphrased request against the frontmatter. A
  description that restates the workflow name gives it nothing to match, so the
  workflow's own words are followed by a trigger sentence, and a workflow whose
  author wrote no description still gets one.
- **A concrete call example.** Placeholders are type-shaped (`<pr>`, `0`,
  `false`, `{}`) rather than plausible-looking invented values, because a model
  that copies an invented value ships it to a real run. Inputs with defaults
  show the default, so the example is runnable as written.
- **A "while it runs" paragraph.** `workflow_run` blocks until the run finishes.
  Saying so is what stops a model calling it again a minute later.
- **A fallback paragraph.** Skills are copied into user-scope directories that
  outlive any MCP configuration, so the tool being absent is a normal state. The
  body says: do not claim the run started, run `medulla workflow run` from the
  shell, or attach the server with `medulla skills install --with-mcp`. A skill
  that dead-ends when the server is missing is worse than no skill.

## The marker discipline

Every generated file's first line is

```text
<!-- medulla:managed workflow=<percent-encoded id> rev=<sha256 of everything below> -->
```

and that line is the entire safety story for writing into someone's `~/.claude`.

The id is percent-encoded, because the store accepts any id that is a single
path component — `nightly sweep`, an id with a newline, a quote, or a `%` in it
— and the marker is a whitespace-separated, single-line field list. Written raw,
`nightly sweep` reads back as `nightly` and a newline pushes `rev` onto line two,
so Medulla stops recognising a file it wrote itself: the reinstall reports a
collision against its own file and `sync --prune` deletes the skill of a live
workflow. Decoding is exact and refuses anything the encoder could not have
produced; a marker it cannot fully account for is treated as someone else's.

`install` reads the file at the target path before writing and takes one of five
decisions: absent → created; marked as ours with a matching `rev` → unchanged,
nothing written; marked as ours with a different `rev` → updated; no marker (or
bytes that are not even UTF-8) → `skippedUnmanaged`, bytes untouched; **marked
for a different workflow → `slugCollision`**, bytes untouched.

The last two are kept apart because they send the operator to different places.
A hand-written `~/.claude/skills/medulla-babysit/SKILL.md` is a file the operator
authored and we do not get to decide its contents. A file carrying *our* marker
for another workflow is Medulla's own doing: slugs are a lossy view of ids, and
`deploy.prod` and `deploy-prod` both slugify to `medulla-deploy-prod`. Reporting
that as unmanaged would blame a third party for a clash the operator can only
fix by renaming one of their workflows. Two workflows in one run that want the
same path collide the same way — the first claim wins and the loser is named —
which is also what keeps `--dry-run` honest: it must reach the identical verdict
the real run does, and a dry run that wrote nothing would otherwise report both
as `created`.

`InstallReport::has_collisions` is true for either kind, so a caller can say the
loud part: the workflow they asked for is *not* installed. Overwriting would be
a silent, unrecoverable edit to a file we never wrote, which is not a trade the
convenience of one fewer error message can justify.

The same rule runs backwards. `sync --prune` and `uninstall` scan for marked
files and delete only those; a stray unmarked neighbour in the same directory
survives both. The scan reads markers rather than filenames, so a skill an older
release installed under a different slug is still recognised as ours. It reads
bytes rather than text, too: one non-UTF-8 file in `~/.claude/commands` counts as
"not ours" instead of failing the whole scan and aborting a sync that had nothing
to do with it.

`rev` is a content hash of everything below the marker, which is what makes an
install idempotent across releases that do not touch the template: rerunning
writes nothing and reports `unchanged`, and rewrites happen exactly when the
generated text actually changed. `--dry-run` reaches the identical decisions and
writes nothing at all — not even parent directories, so a dry run leaves no
trace on disk.

## Targets

| target | skill | slash command |
| --- | --- | --- |
| `claude` | `<root>/.claude/skills/<slug>/SKILL.md` | `<root>/.claude/commands/<slug>.md` |
| `codex` | `<root>/.codex/skills/<slug>/SKILL.md` | `<root>/.codex/prompts/<slug>.md` |
| `generic` | `<root>/.medulla/skills/<slug>/SKILL.md` | none |

`<slug>` is `medulla-<sanitised id>`: prefixed so a listing of `~/.claude/skills`
says where these files came from, sanitised to the `[a-z0-9-]` alphabet both
verified harnesses accept.

`<root>` is `$HOME` for user scope and the project directory for project scope,
resolved by the caller and handed in already absolute — the filesystem code
never re-derives it, which is what lets a test point the whole module at a
tempdir. `HOME` is read from a passed-in map rather than the process
environment for the same reason.

`generic` is the escape hatch for a harness we have not verified: a readable
skill under `.medulla/` beats nothing. It has no command file on purpose —
there is no command convention to guess at, and a markdown file nothing reads
is litter.

Command files are opt-in (`--with-commands`). The skill already gives the model
the trigger; the command is for the operator who would rather type
`/medulla-babysit 123`.

With no harness named, the default target set is "the harnesses whose directory
already exists under `<root>`". Creating `~/.codex` for someone who does not use
Codex is the same litter in a different place. An empty result is a legitimate
answer and the CLI says so rather than silently doing nothing.

## Why registration never shells out

A skill without the MCP server attached is inert, so `--with-mcp` registers
`medulla mcp` with the same harnesses the skills went to. That registration is a
config-file merge we perform ourselves, never a `claude mcp add` subprocess.

Shelling out makes the result depend on which CLI version happens to be on
`PATH`, cannot be tested offline, and cannot honour `--dry-run` — a dry run that
spawns a process that writes a file is not a dry run. Merging JSON and TOML
ourselves is inspectable, deterministic, and identical in both modes.

The merge is read-modify-write: other servers, other top-level keys, and
unrelated tables all survive, and when the desired entry is already present the
file is not rewritten at all. That last part matters more than it sounds for
Codex, where the `toml` crate round-trips values but not comments — returning
before the write is what keeps a repeat registration from costing the operator
their file's formatting.

`command` is the absolute path of the running binary (`current_exe`), not the
bare name: a registration is read by a process whose `PATH` we do not control.

Two targets are deliberately not written:

- **Claude at user scope.** Its project registry is a documented, checked-in
  `.mcp.json` we can merge into. Its *user* registry lives inside the CLI's own
  state, whose shape and location are an implementation detail that has already
  moved once. Writing a file we guessed at would report success while producing
  a registration Claude never reads, so the outcome is `manual` and carries the
  exact `claude mcp add` line to run.
- **`generic`**, for the same reason with less information.

A config that exists but does not parse is reported as `skipped` with the reason
rather than replaced, and rather than aborting the other targets.

## Contents

- [`mod.rs`](./mod.rs) — Harness-native skills that trigger saved workflows over MCP.
- [`types.rs`](./types.rs) — Targets, scopes, rendered skills, and install outcomes.
- [`render.rs`](./render.rs) — `WorkflowSummary` → skill text, and the managed marker.
- [`targets.rs`](./targets.rs) — Where each harness expects to find a skill.
- [`install.rs`](./install.rs) — Install, sync, uninstall, and the marker discipline.
- [`registration.rs`](./registration.rs) — Merging `medulla mcp` into each harness's config.
- [`tests.rs`](./tests.rs) — Rendering, target layout, and managed-file behaviour.
- [`registration_tests.rs`](./registration_tests.rs) — Config merges, preservation, and manual outcomes.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data
structures in `types.rs`, focused unit tests in `tests.rs`, and preserve the
module-level Rust documentation as the API source of truth. New harnesses
belong in `targets.rs` and `registration.rs` — if a change to support one
reaches `render.rs`, the body has stopped being target-independent and that is
worth arguing about first.
