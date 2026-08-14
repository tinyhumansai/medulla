---
description: >-
  How the vendored OpenHuman core and its crates are consumed, and the rules a
  source build depends on.
---

# Vendoring

Some upstream crates are consumed from a git submodule under `vendor/` rather
than from crates.io. The workspace `exclude = ["vendor", "worktrees"]` keeps them
out of `members`, so they carry their own lints and tests instead of joining this
repository's CI gates.

## Initialize

```sh
make init                      # runs the script below, plus tooling and hooks
bash scripts/init-submodules.sh
```

Use the script, never `git submodule update --init --recursive`.
`vendor/openhuman` has submodules of its own, and two of them
(`app/src-tauri/vendor/tauri-cef`, a Tauri fork bundling CEF, and
`app/src-tauri/vendor/tauri-plugin-notification`) belong to the OpenHuman desktop
app. `--recursive` clones both, and nothing in Medulla's graph references either:
the Cargo workspace excludes `vendor/`, so `app/src-tauri` is not a member. The
wiki documentation submodules and `tinycortex`'s own nested `tinyagents` copy are
skipped for the same reason.

A git dependency on OpenHuman would not help. Cargo updates git-dependency
submodules recursively with no opt-out, which makes the CEF clone mandatory. The
submodule plus an explicit init list is the only way to avoid it.

The script initializes `vendor/openhuman` and then several of its own vendored
crates: `tinyagents`, `tinybus`, `tinychannels`, `tinycortex`, `tinyflows`,
`tinyhumans-sdk`, `tinymemory`, and `tinyplace`. `vendor/motosan-ai-oauth` is
not a submodule — OpenHuman vendors it as a plain tracked directory, so it
comes along with the `vendor/openhuman` checkout itself.

## How each dependency is declared

There is exactly one submodule, `vendor/openhuman`
([`tinyhumansai/openhuman`](https://github.com/tinyhumansai/openhuman)). Everything
else vendored reaches the build through it.

| Crate | Declared as |
| --- | --- |
| `openhuman` | path dependency on `vendor/openhuman`, `default-features = false`, never patched (it has no registry coordinate) |
| `tinyhumans-sdk` | path dependency on `vendor/openhuman/vendor/tinyhumans-sdk`, for the same reason |
| `medulla-link` | path dependency on `src/link`; it is ours and is not vendored |
| `tinyagents`, `tinybus`, `tinychannels`, `tinycortex`, `tinycortex-api`, `tinyflows`, `tinymemory`, `tinyplace`, `motosan-ai-oauth` | registry coordinates redirected by `[patch.crates-io]` to `vendor/openhuman/vendor/*` |

Declare each vendored crate exactly one way: either as a direct path dependency
or as a registry coordinate redirected by the patch table, and never both for the
same crate. Mixing the two styles yields two `PackageId`s for one crate and an
`E0308` where the types look identical, the first time a value crosses the
Medulla and OpenHuman seam. Guard with `cargo tree -d`, which must report no
duplicate `tiny*`.

## The root patch table is load-bearing

`[patch.crates-io]` applies only from the workspace root. Once OpenHuman is a
path dependency, its own patch table is ignored, as is
`vendor/openhuman/vendor/tinycortex/.cargo/config.toml`, which is CWD-scoped.
This workspace's root manifest therefore reproduces OpenHuman's entire table with
paths rewritten to `vendor/openhuman/vendor/*`. Drop an entry and Cargo quietly
resolves the published crate instead of the vendored tree rather than failing.

Keep the table in lockstep with `scripts/init-submodules.sh`.

The `tinyplace` entry stays even though no crate in this workspace links
`tinyplace` any more: the vendored OpenHuman core still does, and without the
redirect Cargo would resolve the published crate.

One entry from OpenHuman's table is deliberately not copied, its `whisper-rs-sys`
git patch. Cargo fetches a git patch source during resolution even when the
patched crate is absent from the graph, which would put a network fetch in the
critical path of every `--offline` build and of the `--network none` end-to-end
image. Medulla never enables OpenHuman's `inference` feature, so `whisper-rs` is
not in the graph.

Verify the result with `cargo tree -i tinyagents`: the source must read
`path+file://...`, never `registry+...`.

## The OpenHuman feature set

`openhuman` is declared `default-features = false` with the `medulla` feature
enabled, and both halves of that matter.

Turning defaults off is load-bearing rather than tidiness. OpenHuman's defaults
include its own `tui` gate on ratatui 0.30 and crossterm 0.29 against this
workspace's 0.29 and 0.28. Those are semver-incompatible at 0.x, so Cargo would
link both: two raw-mode state machines and two background reader threads on one
tty file descriptor. That is not a compile error, it is a wrecked terminal.

An empty feature list would compile a core with no Medulla domain at all.
`openhuman::medulla` and the `embed::Medulla` facade both sit behind the
`medulla` gate, so without it the embedded core builds fine and has nothing this
host can call.

The core is not optional. It is the runtime this SDK hosts; a build without it
has no runtime to offer but the offline mock.

## `tinyflows`

`tinyflows` is the DAG workflow engine behind the SDK's `workflows` feature. It
is declared in the root `Cargo.toml` as a registry dependency and redirected to
the vendored tree:

```toml
[workspace.dependencies]
tinyflows = { version = "0.6", features = ["mock"] }

[patch.crates-io]
tinyflows = { path = "vendor/openhuman/vendor/tinyflows" }
```

This is the same shape the sibling `openhuman` host uses. Keeping the registry
coordinate rather than a bare path dependency means the two hosts share one pin
and one upgrade cadence.

The `mock` feature is a normal dependency feature, not a dev-only one: the
authoring surface dry-runs graphs against the engine's deterministic capability
stand-ins in ordinary builds, not just in tests.

Because `tinyflows` is pre-1.0 and still changing its `engine` entry points,
expect the adapter seam in [`src/sdk/src/flow_engine/`](../../src/sdk/src/flow_engine/)
to need attention on an update; the rest of the SDK should not.

## Pins and upgrades

When the two repositories disagree on a shared pin, the newer pin wins and the
bump lands in OpenHuman. Adopting an older OpenHuman pin wholesale is a hard
compile break (for example, a `tinyplace` ancestor missing `signal::maintain`,
which `src/sdk/src/daemon/transport/mod.rs` calls), so advance the pin in
OpenHuman and let this gitlink follow rather than patching around it here.

To move a vendored crate forward:

```sh
cd vendor/openhuman/vendor/tinyflows
git fetch origin
git checkout <new-sha>
cd ../../../..
cargo build            # confirm the adapter seam still compiles
cargo test
```

Then commit the gitlink.

OpenHuman must never depend on the `medulla` crate. Its default-on
`medulla-local` feature currently has an empty dependency list. The day someone
gives it a real edge, `medulla-public` to `openhuman` to `medulla` becomes a
Cargo dependency cycle and a hard failure.

## Coverage and the vendored tree

Coverage excludes the vendored tree. `vendor/` path dependencies are local
packages under the workspace root, so `cargo-llvm-cov`'s default registry filter
does not drop them; the gate's `--ignore-filename-regex` starts with
`(^|/)vendor/` for that reason. Removing it would measure the embedded OpenHuman
core instead of this repository, and the gate would report roughly the
first-party share of a very large tree. See [Testing](testing.md#coverage).

## Read next

* [Getting Started](getting-started.md): building from source.
* [Contributing](contributing.md): the development loop and release process.
* [Testing](testing.md): the suites and the coverage gate.
