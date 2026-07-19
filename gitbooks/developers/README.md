---
description: >-
  Build with Medulla — install the TUI, embed the Rust SDK, and wire your own
  fleet to the orchestrator. Start here.
---

# Developers

This is the developer home for Medulla: how to install and run the terminal app,
how to embed the SDK in your own Rust code, how it is put together, and how to
build the repository from source.

The [product overview](../README.md) is the high-level story; these pages are the
hands-on detail. Everything here tracks the public
[`tinyhumansai/medulla`](https://github.com/tinyhumansai/medulla) repository — a
two-crate Cargo workspace: the [`medulla`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk)
SDK library and the [`medulla-tui`](https://github.com/tinyhumansai/medulla/tree/main/src/tui)
app crate, which ships the `medulla` binary.

## What's here

* [Getting Started](getting-started.md) — install the prebuilt binary or build
  from source, log in, and run your first session.
* [CLI Reference](cli-reference.md) — every `medulla` subcommand: the TUI, the
  headless daemon, the harness wrappers, and self-update.
* [Configuration](configuration.md) — the Medulla home directory, the layered
  config system, and the three runtimes.
* [Authentication](authentication.md) — the browser loopback login flow, tokens,
  and how credentials are stored and hardened.
* [Architecture](architecture.md) — how the SDK and TUI fit together, the runtime
  adapters, the RLM-backed orchestration loop, and the tiny.place bridge.
* [Contributing](contributing.md) — build, test, lint, coverage, and the release
  process.

## The 60-second version

Install the prebuilt binary (it downloads the release asset for your platform,
verifies its SHA-256 against the release manifest when a checksum tool is
available, and installs to `~/.medulla/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

If the installer updated your `PATH`, reload your shell (`exec $SHELL`, or open a
new terminal) so `medulla` resolves. Then log in and start the TUI:

```sh
medulla login   # browser OAuth; stores a verified JWT
medulla         # bare invocation starts the TUI
```

No credentials? `medulla` opens a login screen — press `m` there to explore the
interface offline against the scripted [mock runtime](configuration.md#runtimes),
with no network and no account. See [Getting Started](getting-started.md) for the
full walkthrough.

## Open by design

The orchestrator model is gated, but the tooling around it is not. The SDK, the
TUI, and every benchmark fixture are open source, so you can see exactly how your
harnesses talk to the orchestrator — and reproduce every published number —
before you ever request access. See
[Open Benchmarks, Open SDKs](../open-benchmarks-open-sdks.md).
