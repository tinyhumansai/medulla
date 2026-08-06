# Harness Hooks

Standardized lifecycle hooks for Medulla-launched harnesses: declare a hook once
in Medulla's `[[hooks]]` config — or on the TUI's Hosts → Hooks page — and
Medulla installs it into whichever coding CLI it spawns.

Medulla also installs hooks of its own, on by default, that report each
harness's lifecycle back to it. Without them a harness on a pty is opaque:
"did that turn finish?" is answerable only by pattern-matching a screen.

## Contents

- [`mod.rs`](./mod.rs) — Module docs, the per-spawn [`hook_injection`] entry
  point, and [`launch_args`], which merges hooks with commit attribution because
  Claude Code carries both through one `--settings` flag.
- [`types.rs`](./types.rs) — The canonical event vocabulary, one declared hook,
  the one-line editor form the Hooks page reads and writes, and the injection a
  translator produces. Events deserialize from both their canonical `PascalCase`
  name and its `camelCase` spelling, since the latter is what every other config
  key teaches; see [`HookEvent`]'s own docs.
- [`builtin.rs`](./builtin.rs) — Medulla's own reporting hooks: which events they
  cover, why the deciding events (`PreToolUse`, `PermissionRequest`) are
  deliberately not among them, and how the command reaches the launching binary.
- [`grant.rs`](./grant.rs) — `seed_hook_grant`, the credential every spawn door
  seeds so the built-in's `medulla hook <Event>` command has something to
  report to; see "Medulla's own hooks" below for why every door needed it, not
  only the one that installs the command.
- [`report.rs`](./report.rs) — One report's shape and the bounded log it lands
  in, plus the rule that a summary travels and a payload never does.
- [`native.rs`](./native.rs) — The hook document both Claude Code and Codex
  accept, plus the inline-TOML encoder Codex's `-c` override needs.
- [`claude.rs`](./claude.rs) — Claude Code delivery via `--settings`.
- [`codex.rs`](./codex.rs) — Codex delivery via `-c hooks=…` and the trust bypass.
- [`tests.rs`](./tests.rs) — Vocabulary, document folding, and the exact flags and
  JSON/TOML spelling each harness was verified against.

## Medulla's own hooks

Resolved at config load, ahead of the operator's own, and switched off with
`[hookDefaults] enabled = false` (or `b` on the Hooks page). Each runs
`medulla hook <Event>`, which reads the harness's native payload on stdin and
files a one-line summary on the control socket that spawn was already handed.

Resolving the command at load only gets it *installed* everywhere; it still
needs a socket and a grant to report to. Every non-ACP spawn door now calls
[`seed_hook_grant`](./grant.rs) at its own spawn seam — the operator-started
pty pane, the executor's dispatched pty sessions, the headless daemon's
one-shot runs, the interactive session, and the `medulla <cli>` wrapper — so
each seeds the environment variables the shim reads
(`MEDULLA_HOOK_SOCKET`/`MEDULLA_HOOK_GRANT`) with a hook-only grant scoped to
that one spawn (see [`Grant::hook_only`](../control_socket/grants.rs)). Before
that, only the pty pane's grant was seeded, so a hook installed everywhere else
spawned, found nothing to report to, and exited — pure overhead, and a
meaningfully wasteful one on `PostToolUse`, which fires once per tool call.
**ACP dispatch remains the one door that cannot carry hooks at all**: Medulla
spawns an ACP *server*, which spawns the harness itself, so there is no argv
to install the command onto in the first place (see
`daemon::providers::execute::run_provider_task`'s own comment). A configured
hook there is reported once, as a warning, rather than silently doing nothing.

Two properties are load-bearing:

- They only **observe**. `PreToolUse` and `PermissionRequest` can deny a call, so
  no built-in attaches to them: a hook Medulla installs everywhere must not be
  able to change what a session does.
- The **payload never travels**. Prompt text, tool inputs, and file contents stay
  in the harness's own process tree; the shim summarizes at the source.

They are also never written to an operator's config file, so a later release can
change or withdraw them.

## Why this exists

The same policy — "checkpoint after every edit", "refuse writes outside the
worktree" — otherwise has to be written once per harness, in a different config
language each time, and drifts as soon as one is edited.

Claude Code 2.1.221 and Codex 0.146 converged on the same event names, the same
`matcher` + `hooks[]` grouping, and the same `{"type":"command","command":…}`
handler, so the shared document is built once and only its *delivery* differs.
`Notification` is the sole vocabulary divergence (Claude Code only).

## The operator's side

`config.example.toml`'s `[[hooks]]` section is the reference an operator reads,
and `src/sdk/tests/feature_hooks_config.rs` pins it: the example blocks printed
there are parsed and asserted to reach the harness argv, so the documentation
cannot drift from what actually loads.

## Verification

Both delivery paths were checked end-to-end against those versions using the
exact argv `launch_args` produces: a `SessionStart` hook fires under both, and
under Codex it fires alongside the operator's own `~/.codex/hooks.json` rather
than replacing it.

Two behaviours are load-bearing and easy to regress silently:

- Claude Code accepts **one** `--settings`; a second replaces the first. Hooks and
  attribution therefore share a single merged object.
- Codex **silently skips** hooks absent from its trust store, which a per-spawn
  injection always is, so a Medulla hook does nothing there until the operator
  trusts it once (`/hooks`). Medulla deliberately does not pass
  `--dangerously-bypass-hook-trust` — it is invocation-wide and would also
  authorize hooks the workspace ships in its own `.codex/hooks.json`. Because
  that means the feature is inert until trusted, the requirement is reported as
  a warning rather than left silent.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data
structures in `types.rs`, focused unit tests in `tests.rs`, and preserve the
module-level Rust documentation as the API source of truth.
