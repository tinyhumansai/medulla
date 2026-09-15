---
description: >-
  Build with Medulla: install the TUI, embed the Rust SDK, and wire your own
  fleet to the orchestrator.
---

# Overview

This is the developer home for Medulla: how to install and run the terminal app, how to embed the SDK in your own Rust code, how it is put together, and how to build the repository from source.

The [product overview](../) is the high-level story; these pages are the hands-on detail. Everything here tracks the [`tinyhumansai/medulla-src`](https://github.com/tinyhumansai/medulla-src) repository, a three-crate Cargo workspace: the [`medulla`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/) SDK library and the [`medulla-tui`](https://github.com/tinyhumansai/medulla-src/tree/main/src/tui/) app crate, which ships the `medulla` binary.

## Read next

| Page | What it covers |
| --- | --- |
| [Getting Started](getting-started.md) | Install the prebuilt binary on any platform or build from source, log in, and run your first session. |
| [The TUI](the-tui.md) | The tabs, hosting work on this device, adding another machine, and steering a running fleet. |
| [CLI Reference](cli-reference.md) | Every `medulla` subcommand: the TUI, the worker daemon, workflows, the harness wrappers, and self-update. |
| [Configuration](configuration.md) | The Medulla home directory, the layered config system, local state, and the runtimes. |
| [Environment Variables](environment-variables.md) | Every variable Medulla reads, what it does, and its default. |
| [Authentication](authentication.md) | The browser loopback login flow, tokens, and how credentials are stored and hardened. |
| [Troubleshooting](troubleshooting.md) | Install, login, hosting and enrollment failures, clipboard through tmux and SSH, and where the logs are. |
| [Architecture](architecture.md) | How the SDK and TUI fit together, the runtime adapters, sessions, workflows, and the host-link bridge. |
| [Glossary](glossary.md) | The vocabulary of the system, from orchestrator and agent through ledger, budget, and flavor. |
| [The Rust SDK](sdk.md) | The `medulla` crate: cargo features, the `Runtime` trait, the backend client, examples, and the module tree. |
| [Harness Integration](harness-integration.md) | The public harness wire contract, the ACP stdio transport, and the shared-process Codex path. |
| [Attribution and Routing](attribution-and-routing.md) | The loopback proxy that rewrites OpenRouter attribution and keeps the provider key out of the harness. |
| [Host Link Protocol](host-link-protocol.md) | The normative `medulla-link/1` wire specification, its forwarder rules, and its conformance tests. |
| [Testing](testing.md) | Where tests live, the shared stand-ins, the offline and live suites, and the coverage gate. |
| [Vendoring](vendoring.md) | The three vendored crates, the patch table, and the rules a source build depends on. |
| [Contributing](contributing.md) | Build, test, lint, coverage, and the release process. |

## Install and run

Install the prebuilt binary. It downloads the release asset for your platform, verifies its SHA-256 against the release manifest, and installs to `~/.medulla/bin`:

```sh
# macOS and Linux
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

If the installer updated your `PATH`, open a new terminal (or run `exec $SHELL`) so `medulla` resolves. Then log in and start the TUI:

```sh
medulla login   # browser OAuth; stores a verified JWT
medulla         # bare invocation starts the TUI
```

With no credentials configured, `medulla` opens a login screen. Press `m` there to explore the interface offline against the scripted [mock runtime](configuration.md#runtimes), with no network and no account. See [Getting Started](getting-started.md) for the full walkthrough.

## What is open source

The SDK and the TUI are open source; the orchestrator backend is gated. You can read how the harnesses talk to the orchestrator, and run the whole client offline against the mock runtime, before requesting access. See the [product overview](../) for access details.
