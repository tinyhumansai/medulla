# Medulla — source

**One terminal. Every agent you have. Working at once.**

This is the Rust workspace behind Medulla: the [`medulla`](src/sdk) SDK crate
and the [`medulla-tui`](src/tui) crate that ships the `medulla` binary, both
under GPL-3.0-only.

The product's public surface lives in a separate repository,
[**tinyhumansai/medulla**](https://github.com/tinyhumansai/medulla) — the
documentation, the GitBook sources, `install.sh` / `install.ps1`, and every
release. This repository builds the binaries; the `Release` workflow publishes
them there. No source crosses that boundary.

## Install (users)

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

Then `medulla login` to sign in, or `medulla --mock` for a full offline demo
with no account and no network.

Full documentation lives at
**[tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla)**.

## Build (developers)

```sh
make init                       # submodules, rustfmt/clippy, locked deps, pre-push hook
cargo run                       # debug build, starts the TUI (login screen if signed out)
cargo run -- --mock             # the offline demo runtime
cargo test                      # unit + feature + e2e suites (all mocked, no network)
make ci                         # the complete pre-push gate
```

See [DEVELOPING.md](DEVELOPING.md) for the developer documentation map and the
release process, and
[Contributing](https://tinyhumans.gitbook.io/medulla/developers/contributing)
for the full loop.

Configuration is documented inline in
[`config.example.toml`](config.example.toml), which carries the complete shape
of every section.
