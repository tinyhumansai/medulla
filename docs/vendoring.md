# Vendored dependencies

Some upstream crates are consumed as git submodules under `vendor/` rather than
from crates.io. The workspace `exclude = ["vendor"]` keeps them out of `members`,
so they carry their own lints and tests instead of joining this repository's CI
gates.

| Submodule | Upstream | Consumed as |
| --- | --- | --- |
| `vendor/tinyplace` | `tinyhumansai/tiny.place` | path dependency |
| `vendor/tinycortex` | `tinyhumansai/tinycortex` | path dependency |
| `vendor/tinyflows` | `tinyhumansai/tinyflows` | registry coordinate + `[patch.crates-io]` |

Initialize everything with:

```sh
git submodule update --init --recursive
```

## tinyflows

`tinyflows` is the DAG workflow engine behind the `workflows` feature. It is
declared in the root `Cargo.toml` as a *registry* dependency and redirected to
the submodule:

```toml
[workspace.dependencies]
tinyflows = { version = "0.5", features = ["mock"] }

[patch.crates-io]
tinyflows = { path = "vendor/tinyflows" }
```

This is the same shape the sibling `openhuman` host uses. The indirection exists
because crates.io lags the branch we track — the published maximum is `0.3.0`
while the pinned tree reports `0.5.1` — so a plain registry dependency would not
resolve to the code we build against. Keeping the registry coordinate (rather
than a bare path dependency) means the two hosts share one pin and one upgrade
cadence.

**Pinned commit:** `fb24363aea921f957958bc8f4aeb5b0a244e41c7` (`v0.3.0-37-gfb24363`),
matching `openhuman`.

The `mock` feature is a normal dependency feature, not a dev-only one: the
authoring surface dry-runs graphs against the engine's deterministic capability
stand-ins in ordinary builds, not just in tests.

### One `tinyagents`, and the patch entry is mandatory

This section used to say the opposite. Before the OpenHuman repoint the graph
resolved **two** distinct `tinyagents` packages:

- `2.0.0` — a path dependency vendored inside `vendor/tinycortex/vendor/tinyagents`
- `2.1.0` — from crates.io, required by `tinyflows`

and the guidance was "do not add a `[patch.crates-io]` entry for `tinyagents`".

That is now **inverted**. OpenHuman's `vendor/tinycortex` requires
`tinyagents = "2.1"`, so sourcing `tinycortex` from there collapses the graph to
a single `tinyagents` and the patch entry is **mandatory**. Omitting it does not
fail loudly: `tinyagents 2.1.0` is published on crates.io, so the build silently
resolves the registry copy instead of the vendored tree (~14 commits ahead).

Verify with `cargo tree -i tinyagents` — the source must read `path+file://…`,
never `registry+…`. And `cargo tree -d` must report no duplicate `tiny*`.

## Vendored OpenHuman core

`vendor/openhuman` carries the OpenHuman core that medulla embeds. Two things
about it are load-bearing and easy to get wrong.

**Initialize with `scripts/init-submodules.sh`, never `--recursive`.**
`vendor/openhuman` has submodules of its own, two of which belong to the
OpenHuman *desktop* app — including a Tauri fork that bundles CEF. `--recursive`
clones both, and nothing in medulla's graph references either (the Cargo
workspace excludes `vendor/`, so `app/src-tauri` is not a member). A git
dependency would not help: Cargo updates git-dependency submodules recursively
with no opt-out, which makes the CEF clone mandatory. The submodule plus an
explicit init list is the only way to avoid it.

**The root `[patch.crates-io]` table is load-bearing.** `[patch.crates-io]`
applies only from the workspace root, so once OpenHuman is a path dependency
*its* patch table is ignored — as is
`vendor/openhuman/vendor/tinycortex/.cargo/config.toml`, which is CWD-scoped.
This workspace's root manifest must therefore reproduce OpenHuman's entire table
with paths rewritten to `vendor/openhuman/vendor/*`. Drop an entry and Cargo
quietly resolves the published crate instead of the vendored tree.

**Declare each vendored crate exactly one way.** Either a direct path dependency
or a registry coordinate redirected by the patch table — never both for the same
crate. Mixing the two styles yields two `PackageId`s for one crate and an
`E0308` where the types look identical, the first time a value crosses the
medulla↔OpenHuman seam. Guard with `cargo tree -d`, which must report no
duplicate `tiny*`.

**Newer pin wins, and OpenHuman is where the bump lands.** The two repos
disagreed on `tinyplace`: OpenHuman was an ancestor missing `signal::maintain`,
which `src/sdk/src/daemon/transport/mod.rs` calls. Adopting OpenHuman's pin
wholesale is a hard compile break, so the fix is to advance the pin *in
OpenHuman* and let this gitlink follow — not to patch around it here.

**OpenHuman must never depend on the `medulla` crate.** Its default-ON
`medulla-local` feature currently has an empty dependency list. The day someone
gives it a real edge, `medulla-public → openhuman → medulla` becomes a Cargo
dependency cycle and a hard failure.

**Coverage excludes the vendored tree.** `vendor/` path deps are *local*
packages under the workspace root, so `cargo-llvm-cov`'s default registry filter
does not drop them; the gate's `--ignore-filename-regex` starts with
`(^|/)vendor/` for that reason. Removing it sinks the 95% gate to roughly the
first-party share of a very large tree.

### Updating the pin

```sh
cd vendor/tinyflows
git fetch origin
git checkout <new-sha>
cd ../..
cargo build            # confirm the adapter seam still compiles
cargo test
```

Then update the pinned commit recorded above and commit the gitlink. Because
`tinyflows` is pre-1.0 and still changing its `engine` entry points, expect the
adapter seam in `src/sdk/src/tinyflows/` to need attention on an update; the rest
of the SDK should not.
