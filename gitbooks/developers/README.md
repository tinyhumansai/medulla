---
description: >-
  Run Medulla yourself: install the binary, configure remote hosts, embed the
  Rust SDK, and build from source.
---

# Overview

This is the developer home for Medulla: how to install and run the terminal, how it is configured, how it is put together, and how to build it from source.

The [product overview](../) is the short story; these pages are the detail. Everything here tracks the [`tinyhumansai/medulla-src`](https://github.com/tinyhumansai/medulla-src) repository, a three-crate Cargo workspace: the [`medulla`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/) SDK library, the [`medulla-tui`](https://github.com/tinyhumansai/medulla-src/tree/main/src/tui/) app crate that ships the `medulla` binary, and [`medulla-link`](https://github.com/tinyhumansai/medulla-src/tree/main/src/link/), the host link transport.

## Read next

| Page | What it covers |
| --- | --- |
| [Getting Started](getting-started.md) | Install the prebuilt binary on any platform or build from source, log in, and open your first session. |
| [The TUI](the-tui.md) | The tabs, every key, the session picker, and the settings pages. |
| [CLI Reference](cli-reference.md) | Every `medulla` subcommand: the TUI, the daemon, `remote`, workflows, skills, and self-update. |
| [Configuration](configuration.md) | The Medulla home directory, the layered config, and every section of `config.toml`. |
| [Environment Variables](environment-variables.md) | Every variable Medulla reads, what it does, and its default. |
| [Authentication](authentication.md) | The browser loopback login, the paste-a-code flow for SSH, tokens, and where credentials are stored. |
| [Troubleshooting](troubleshooting.md) | Install, login, and remote host failures, clipboard through tmux and SSH, and where the logs are. |
| [Architecture](architecture.md) | How the SDK, the TUI, the PTY layer, and the host link fit together. |
| [Glossary](glossary.md) | The vocabulary: session, harness, host, remote host, daemon, host link, workflow. |
| [The Rust SDK](sdk.md) | The `medulla` crate: cargo features, the `Backend` trait, examples, and the module tree. |
| [Harness Integration](harness-integration.md) | How a coding CLI is launched, the ACP transport, and the shared-process Codex path. |
| [Attribution and Routing](attribution-and-routing.md) | The loopback proxy that keeps the provider key out of the harness and routes through a gateway. |
| [Host Link Protocol](host-link-protocol.md) | The normative `medulla-link/1` wire specification and its conformance tests. |
| [Testing](testing.md) | Where tests live, the offline and live suites, and the coverage gate. |
| [Vendoring](vendoring.md) | The vendored crates, the patch table, and the rules a source build depends on. |
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

With no credentials configured, `medulla` opens a login screen. To explore offline instead, run `medulla --mock`, the scripted [mock runtime](configuration.md#the-mock-runtime), with no network and no account. See [Getting Started](getting-started.md) for the full walkthrough.

## What is open source

The SDK, the TUI, and the host link are open source under GPL-3.0-only. The hosted sign-in and the account behind it are what a plan pays for. You can read all of the client, build it, and run it offline against the mock runtime without one. See [Availability](../availability.md).
