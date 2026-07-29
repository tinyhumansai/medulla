---
description: >-
  Build with Medulla — install the TUI, embed the Rust SDK, and wire your own
  fleet to the orchestrator. Start here.
---

# Overview

This is the developer home for Medulla: how to install and run the terminal app, how to embed the SDK in your own Rust code, how it is put together, and how to build the repository from source.

The [product overview](../) is the high-level story; these pages are the hands-on detail. Everything here tracks the public [`tinyhumansai/medulla`](https://github.com/tinyhumansai/medulla) repository — a two-crate Cargo workspace: the [`medulla`](../../src/sdk/) SDK library and the [`medulla-tui`](../../src/tui/) app crate, which ships the `medulla` binary.

## What's here

* [Getting Started](getting-started.md) — install the prebuilt binary on any platform or build from source, log in, and run your first session.
* [The TUI](the-tui.md) — the tabs, hosting work on this device, adding another machine, and steering a running fleet.
* [CLI Reference](cli-reference.md) — every `medulla` subcommand: the TUI, the worker daemon, workflows, the harness wrappers, and self-update.
* [Configuration](configuration.md) — the Medulla home directory, the layered config system, local state, and the runtimes.
* [Authentication](authentication.md) — the browser loopback login flow, tokens, and how credentials are stored and hardened.
* [Architecture](architecture.md) — how the SDK and TUI fit together, the runtime adapters, sessions, workflows, and the tiny.place bridge.
* [Contributing](contributing.md) — build, test, lint, coverage, and the release process.

## The 60-second version

Install the prebuilt binary. It downloads the release asset for your platform, verifies its SHA-256 against the release manifest, and installs to `~/.medulla/bin`:

```sh
# macOS and Linux
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

If the installer updated your `PATH`, open a new terminal — or run `exec $SHELL` — so `medulla` resolves. Then log in and start the TUI:

```sh
medulla login   # browser OAuth; stores a verified JWT
medulla         # bare invocation starts the TUI
```

No credentials? `medulla` opens a login screen — press `m` there to explore the interface offline against the scripted [mock runtime](configuration.md#runtimes), with no network and no account. See [Getting Started](getting-started.md) for the full walkthrough.

## Open by design

The orchestrator is gated, but the tooling around it is not. The SDK and the TUI are open source, so you can read exactly how your harnesses talk to the orchestrator — and run the whole thing offline against the mock runtime — before you ever request access.
