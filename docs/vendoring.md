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

### Two `tinyagents` copies is deliberate

The dependency graph resolves **two** distinct `tinyagents` packages:

- `2.0.0` — a path dependency vendored inside `vendor/tinycortex/vendor/tinyagents`
- `2.1.0` — from crates.io, required by `tinyflows`

They are separate packages to Cargo, so their traits are separate identities.
That is correct here: no value crosses between the persona stack and the workflow
engine, and `tinyflows` needs `2.1` APIs that the vendored `2.0.0` copy does not
have. Do not add a `[patch.crates-io]` entry for `tinyagents` — it would force
the engine onto an older tree and fail to build. Revisit only if a type ever has
to pass between `tinycortex` and `tinyflows` directly.

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
