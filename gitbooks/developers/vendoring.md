---
description: >-
  How the three vendored crates under vendor/ are consumed, and the rules a
  source build depends on.
---

# Vendoring

Three upstream crates are consumed from git submodules under `vendor/` rather
than from crates.io. The workspace `exclude = ["vendor", "worktrees"]` keeps them
out of `members`, so they carry their own lints and tests instead of joining this
repository's CI gates.

| Submodule | Upstream | What it is |
| --- | --- | --- |
| `vendor/tinyagents` | [`tinyhumansai/tinyagents`](https://github.com/tinyhumansai/tinyagents) | The agent harness: the model/tool loop behind the in-process `openhuman` provider |
| `vendor/tinyflows` | [`tinyhumansai/tinyflows`](https://github.com/tinyhumansai/tinyflows) | The DAG workflow engine behind the SDK's `workflows` feature |
| `vendor/tinyhumans-sdk` | [`tinyhumansai/sdk`](https://github.com/tinyhumansai/sdk) | The shared TinyHumans HTTP transport the direct backend client builds on |

All three are self-contained: none declares a path or git dependency of its own,
and none carries code submodules of its own.

> This used to be a much longer page. Until v0.11.0 the runtime was an embedded
> OpenHuman core at `vendor/openhuman`, which carried sixteen submodules of its
> own — two of them a Tauri fork bundling CEF — and forced the root manifest to
> reproduce that core's entire patch table rebased onto `vendor/openhuman/vendor/*`.
> Removing the core removed all of it. See [Architecture](architecture.md).

## Initialize

```sh
make init                          # the script below, plus tooling, deps and hooks
bash scripts/init-submodules.sh    # submodules only
```

Use the script rather than `git submodule update --init --recursive`.
`vendor/tinyagents` carries a `wiki` documentation submodule that nothing here
compiles, and `--recursive` descends unconditionally. That is the whole of the
difference now, but the script is still the one place the vendored set is
written down, and it must stay in lockstep with the root manifest's
`[patch.crates-io]` table.

The submodule URLs in `.gitmodules` are HTTPS, so CI clones them without a
deploy key.

## How each dependency is declared

Declare each vendored crate exactly one way: either as a direct path dependency
or as a registry coordinate redirected by the patch table, and never both for the
same crate. Mixing the two styles yields two `PackageId`s for one crate and an
`E0308` where the types look identical, the first time a value crosses the seam.

| Crate | Declared as |
| --- | --- |
| `tinyagents` | registry coordinate `"2.1"` with `features = ["sqlite", "tools"]`, redirected by `[patch.crates-io]` to `vendor/tinyagents` |
| `tinyflows` | registry coordinate `"0.8"` with `features = ["mock", "host-caps", "store"]`, redirected by `[patch.crates-io]` to `vendor/tinyflows` |
| `tinyhumans-sdk` | plain path dependency on `vendor/tinyhumans-sdk`; it has no registry coordinate, so there is nothing to patch |
| `medulla-link` | path dependency on `src/link`; it is ours and is not vendored |

Neither `tinyagents` feature is on by default in the crate. `sqlite` brings the
durable session store (`tinyagents::session`) that backs the agent's per-thread
transcript history; `tools` brings the builtin tool family the agent loop
dispatches. `tinyflows`'s `mock` is a normal dependency feature rather than a
dev-only one: the authoring surface dry-runs graphs against the engine's
deterministic capability stand-ins in ordinary builds, not just in tests.

Guard the result with two commands:

```sh
cargo tree -d                 # must report no duplicate tiny*
cargo tree -i tinyagents      # source must read path+file://…, never registry+…
```

## The root patch table

`[patch.crates-io]` applies only from the workspace root, so every vendored crate
that also has a registry coordinate must be redirected there. Today that is two
entries:

```toml
[patch.crates-io]
tinyagents = { path = "vendor/tinyagents" }
tinyflows  = { path = "vendor/tinyflows" }
```

Dropping an entry does not fail loudly. Both crates are published, so Cargo
quietly resolves the registry copy instead of the vendored tree and the build
succeeds against the wrong code. Keep the table in lockstep with
`scripts/init-submodules.sh`.

## Pins and upgrades

To move a vendored crate forward:

```sh
cd vendor/tinyflows
git fetch origin
git checkout <new-sha>
cd ../..
cargo build            # confirm the adapter seam still compiles
cargo test
```

Then commit the gitlink.

Because `tinyflows` is pre-1.0 and still changing its `engine` entry points,
expect the adapter seam in
[`src/sdk/src/flow_engine/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/flow_engine/)
to need attention on an update; the rest of the SDK should not. `tinyagents`
upgrades surface in [`src/sdk/src/agent/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/agent/),
and `tinyhumans-sdk` upgrades in
[`src/sdk/src/client/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/).

## Coverage and the vendored tree

Coverage excludes the vendored tree. `vendor/` path dependencies are local
packages under the workspace root, so `cargo-llvm-cov`'s default registry filter
does not drop them; the gate's `--ignore-filename-regex` starts with
`(^|/)vendor/` for that reason. Removing it would measure three upstream crates
instead of this repository. See [Testing](testing.md#coverage).

## Read next

* [Architecture](architecture.md): where each vendored crate sits in the runtime.
* [Getting Started](getting-started.md): building from source.
* [Contributing](contributing.md): the development loop and release process.
* [Testing](testing.md): the suites and the coverage gate.
