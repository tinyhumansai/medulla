# Vendored dependencies

Three upstream crates are consumed as git submodules under `vendor/` rather than
from crates.io. The workspace `exclude = ["vendor", "worktrees"]` keeps them out
of `members`, so they carry their own lints and tests instead of joining this
repository's CI gates.

| Submodule | Upstream | Consumed as |
| --- | --- | --- |
| `vendor/tinyagents` | `tinyhumansai/tinyagents` | registry coordinate `2.1` + `[patch.crates-io]` |
| `vendor/tinyflows` | `tinyhumansai/tinyflows` | registry coordinate `0.8` + `[patch.crates-io]` |
| `vendor/tinyhumans-sdk` | `tinyhumansai/sdk` | path dependency (no registry coordinate) |

Initialize with:

```sh
bash scripts/init-submodules.sh
```

not `git submodule update --init --recursive`. `vendor/tinyagents` carries a
`wiki` documentation submodule that nothing here compiles, and `--recursive`
descends unconditionally. The script is also the one place the vendored set is
written down, and must stay in lockstep with the root manifest's
`[patch.crates-io]` table.

All three submodules are self-contained: none declares a path or git dependency
of its own, and none carries code submodules of its own. `.gitmodules` uses
HTTPS URLs so CI clones them without a deploy key.

## What each one is for

`tinyagents` is the agent harness — the bounded model/tool loop that
`src/sdk/src/daemon/providers/local/` runs in-process for the `openhuman`
harness provider, replacing what used to be an `inference_agent_chat` RPC into
the embedded core. The `sqlite` feature brings `tinyagents::session`, the durable
store behind `src/sdk/src/agent/history/`; `tools` brings the builtin tool family
the loop dispatches. Neither is on by default in the crate.

`tinyflows` is the DAG workflow engine behind the SDK's `flows` feature, reached
through the adapter seam in `src/sdk/src/flow_engine/`. Its `mock` feature is a
normal dependency feature rather than a dev-only one: the authoring surface
dry-runs graphs against the engine's deterministic capability stand-ins in
ordinary builds, not just in tests. `host-caps` and `store` supply the host
capability set and the graph store.

`tinyhumans-sdk` is the shared TinyHumans HTTP transport (`TinyHumansClient`)
that `src/sdk/src/client/` builds the typed Medulla surface on: auth, durable
sessions, SSE event streaming, one-shot orchestration, and the public feedback
board. It owns credential headers, the `{success, data}` envelope, and path
percent-encoding.

## One declaration style per crate

Declare each vendored crate exactly one way: either as a direct path dependency
or as a registry coordinate redirected by the patch table, never both. Mixing the
two yields two `PackageId`s for one crate and an `E0308` where the types look
identical, the first time a value crosses the seam. `tinyhumans-sdk` is a path
dependency because it has no registry coordinate to patch.

`[patch.crates-io]` applies only from the workspace root:

```toml
[patch.crates-io]
tinyagents = { path = "vendor/tinyagents" }
tinyflows  = { path = "vendor/tinyflows" }
```

Dropping an entry does not fail loudly. Both crates are published, so Cargo
silently resolves the registry copy instead of the vendored tree.

Verify with `cargo tree -i tinyagents` — the source must read `path+file://…`,
never `registry+…` — and `cargo tree -d`, which must report no duplicate `tiny*`.

## Updating a pin

```sh
cd vendor/tinyflows
git fetch origin
git checkout <new-sha>
cd ../..
cargo build            # confirm the adapter seam still compiles
cargo test
```

Then commit the gitlink. Because `tinyflows` is pre-1.0 and still changing its
`engine` entry points, expect `src/sdk/src/flow_engine/` to need attention on an
update; the rest of the SDK should not.

## Coverage

Coverage excludes the vendored tree. `vendor/` path dependencies are local
packages under the workspace root, so `cargo-llvm-cov`'s default registry filter
does not drop them; the gate's `--ignore-filename-regex` starts with
`(^|/)vendor/` for that reason.

## History

Until v0.11.0 the runtime was an embedded OpenHuman core vendored at
`vendor/openhuman`. It carried sixteen submodules of its own — two belonging to
the OpenHuman desktop app, including a Tauri fork bundling CEF — and because
`[patch.crates-io]` applies only from the workspace root, this manifest had to
reproduce that core's entire patch table rebased onto `vendor/openhuman/vendor/*`:
ten entries, eight of them pinning a crate nothing here linked directly, each one
silently resolving to a published crate if dropped. `scripts/init-submodules.sh`
had to enumerate the same set by hand because Cargo reported one missing entry
per resolution failure, costing a CI round trip each. Removing the core removed
that whole class of failure; what is left is the three self-contained crates
above.
