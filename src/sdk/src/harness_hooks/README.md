# Harness Hooks

Standardized lifecycle hooks for Medulla-launched harnesses: declare a hook once
in Medulla's `[[hooks]]` config, and Medulla installs it into whichever coding
CLI it spawns.

## Contents

- [`mod.rs`](./mod.rs) — Module docs, the per-spawn [`hook_injection`] entry
  point, and [`launch_args`], which merges hooks with commit attribution because
  Claude Code carries both through one `--settings` flag.
- [`types.rs`](./types.rs) — The canonical event vocabulary, one declared hook,
  and the injection a translator produces. Events deserialize from both their
  canonical `PascalCase` name and its `camelCase` spelling, since the latter is
  what every other config key teaches; see [`HookEvent`]'s own docs.
- [`native.rs`](./native.rs) — The hook document both Claude Code and Codex
  accept, plus the inline-TOML encoder Codex's `-c` override needs.
- [`claude.rs`](./claude.rs) — Claude Code delivery via `--settings`.
- [`codex.rs`](./codex.rs) — Codex delivery via `-c hooks=…` and the trust bypass.
- [`tests.rs`](./tests.rs) — Vocabulary, document folding, and the exact flags and
  JSON/TOML spelling each harness was verified against.

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
