# Getting Started

Medulla ships as a single binary, `medulla`: a [ratatui](https://ratatui.rs/) terminal app that talks to the Medulla backend, watches your agent lanes live, and lets you steer the fleet. This page gets it installed and running.

## Prerequisites

* A real terminal — the TUI refuses to start without a TTY. Kitty-protocol terminals ([kitty](https://sw.kovidgoyal.net/kitty/), [WezTerm](https://wezterm.org/), [Ghostty](https://ghostty.org/), recent iTerm2) additionally get Shift-Enter for newlines in the composer.
* To build from source: Rust stable (edition 2021) via [rustup](https://rustup.rs/).

## Install the prebuilt binary

The install script downloads the release asset for your platform, verifies its SHA-256 against the [release](https://github.com/tinyhumansai/medulla/releases) manifest when a checksum tool (`sha256sum`, `shasum`, or `openssl`) is available — otherwise it warns and skips the check — and installs to `~/.medulla/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

If the installer updated your `PATH`, reload your shell (`exec $SHELL`, or open a new terminal) so the `medulla` command resolves; otherwise invoke it directly as `~/.medulla/bin/medulla`.

* Pin a version: `| sh -s -- X.Y.Z`.
* Change the install prefix: set `MEDULLA_HOME`.
* On a platform without a prebuilt asset the script falls back to `cargo install`.

Prebuilt binaries ship for Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64). See [platform support](getting-started.md#platform-support) for what's unix-only.

## Build from source

```sh
git clone https://github.com/tinyhumansai/medulla
cd medulla
make init                       # submodules, rustfmt/clippy, locked deps, pre-push hook
cargo run                       # debug build, starts the TUI (mock runtime)
cargo run --release             # optimized build
cargo install --path src/tui    # installs the `medulla` binary onto your PATH
```

`make init` initializes vendored submodules, installs the Rustfmt and Clippy components, fetches locked dependencies, and enables the repository's pre-push hook (which checks formatting and runs Clippy with warnings denied). See [Contributing](contributing.md) for the full development loop.

## First run

```sh
medulla login   # browser OAuth loopback flow; stores a verified JWT
medulla         # bare invocation starts the TUI
```

`medulla login` opens your browser, captures the JWT the backend redirects back with, verifies it, and saves credentials under your [Medulla home](configuration.md#medulla-home). The next `medulla` run picks them up automatically. Full detail — providers, headless tokens, and the security model — is in [Authentication](authentication.md).

If you start `medulla` with no working credentials, the TUI shows an in-terminal login screen (browser flow, paste-a-token, or press `m` to continue against the scripted [mock runtime](configuration.md#mock-zero-setup)).

## Explore with zero setup

```sh
cargo run       # or: medulla, with no token configured
```

With no token and no core socket, `medulla` opens a login screen; press `m` to continue offline against the mock runtime — a scripted demo with no credentials and no network, and the fastest way to explore the interface. Open the Settings tab (its Help subpage) or run `/help` for keybindings. Usage, effective config, and the color-theme editor live under Settings as well (`/usage`, `/config`, `/theme`).

## Use the SDK from your own crate

Add the SDK as a git dependency (the repo vendors its path deps, so no extra setup):

```toml
[dependencies]
medulla = { git = "https://github.com/tinyhumansai/medulla", tag = "v0.3.0" }
```

The [`medulla` SDK crate](../../src/sdk/) is a UI-free logic library: the HTTP/SSE client for the backend API, the runtime adapters, persona memory, and the tiny.place integration. See [Architecture](architecture.md) for how the pieces fit together.

## Platform support

Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64) all build and ship prebuilt binaries. The [core-socket runtime](configuration.md#core-socket), the headless [daemon's](cli-reference.md#medulla-daemon) provider-spawn paths, and the [harness wrappers](cli-reference.md#harness-wrappers) are unix-only; the interactive TUI over the backend and mock runtimes, and `medulla update`, work everywhere.

## Next steps

* [CLI Reference](cli-reference.md) — the daemon, harness wrappers, and self-update.
* [Configuration](configuration.md) — home directory, layered config, runtimes.
* [Architecture](architecture.md) — how it all fits together.
