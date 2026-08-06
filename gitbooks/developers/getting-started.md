# Getting Started

Medulla ships as a single binary, `medulla`: a [ratatui](https://ratatui.rs/) terminal app that talks to the Medulla backend, watches your agent lanes live, and lets you steer the fleet. This page gets it installed and running.

## Prerequisites

* A real terminal — the TUI refuses to start without a TTY. Kitty-protocol terminals ([kitty](https://sw.kovidgoyal.net/kitty/), [WezTerm](https://wezterm.org/), [Ghostty](https://ghostty.org/), recent iTerm2) additionally get Shift-Enter for newlines in the composer.
* To build from source: Rust stable (edition 2021) via [rustup](https://rustup.rs/).

## Install the prebuilt binary

Both installers do the same thing: resolve the release manifest, download the asset for your platform, verify its SHA-256, and install to `~/.medulla/bin`. If no prebuilt asset ships for your platform, they fall back to `cargo install`.

**macOS and Linux** — `install.sh`, POSIX `sh`, needs `curl` or `wget` plus `tar`:

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh -s -- 0.5.3   # pin a version
```

The checksum is verified when `sha256sum`, `shasum`, or `openssl` is available; with none of them it warns and skips the check rather than failing.

**Windows** — `install.ps1`, works in PowerShell 7 (`pwsh`) and in the Windows PowerShell 5.1 that ships in the box:

```powershell
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

Installs to `%USERPROFILE%\.medulla\bin` and appends it to your **user** `PATH`, so no elevation is needed. Verification is always on — `Get-FileHash` is built in. To pin a version, the script needs to be on disk so it can take an argument:

```powershell
iwr -useb https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.5.3
```

If a running `medulla.exe` holds a lock on its own image, the installer moves it aside as `medulla.exe.old` rather than failing.

Both scripts honour the same environment variables:

| Variable | Effect |
| --- | --- |
| `MEDULLA_HOME` | Install prefix (default `~/.medulla`). |
| `MEDULLA_NO_MODIFY_PATH=1` | Do not touch shell profiles or the user `PATH`. |
| `MEDULLA_UPDATE_URL` | Override the release manifest URL (for testing). |

If the installer updated your `PATH`, open a new terminal — or run `exec $SHELL` — so the `medulla` command resolves; otherwise invoke it directly from the install prefix.

Prebuilt binaries ship for Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64). See [platform support](#platform-support) for what is unix-only. Every installer is exercised on each of those platforms — plus Fedora, Debian, and Rocky — by the `Install scripts` CI workflow, which installs from the real published release and then runs the binary.

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

With nobody signed in, `medulla` opens a login screen; press `m` to continue offline against the mock runtime — a scripted demo with no credentials and no network, and the fastest way to explore the interface. Open the Settings tab (its Help subpage) or run `/help` for keybindings. Usage, effective config, and the color-theme editor live under Settings as well (`/usage`, `/config`, `/theme`).

## Use the SDK from your own crate

Add the SDK as a git dependency (the repo vendors its path deps, so no extra setup):

```toml
[dependencies]
medulla = { git = "https://github.com/tinyhumansai/medulla", tag = "v0.3.0" }
```

The [`medulla` SDK crate](../../src/sdk/) is a UI-free logic library: the HTTP/SSE client for the backend API, the runtime adapters over the embedded core, sessions, workflows, and the host-link integration. See [Architecture](architecture.md) for how the pieces fit together.

## Platform support

Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64) all build and ship prebuilt binaries. The [daemon's](cli-reference.md#medulla-daemon) provider-spawn paths and the [harness wrappers](cli-reference.md#harness-wrappers) are unix-only; the interactive TUI, `medulla run`, and `medulla update` work everywhere, since the core is embedded rather than reached over a Unix socket.

### The Linux glibc floor

The prebuilt Linux binaries are built on Ubuntu 24.04 and therefore need **glibc 2.39 or newer**. That covers Ubuntu 24.04+, Debian 13+, and current Fedora, but *not* the long-term enterprise distributions — RHEL 9 and its rebuilds (Rocky, AlmaLinux), Debian 12, or Amazon Linux 2023, which ship glibc 2.34-2.36.

On those systems the download would succeed and every later invocation would die on a missing symbol version, so `install.sh` runs the binary once immediately after installing it. If it cannot start, the installer removes it, says why, and falls back to building from source — which works anywhere Rust does. Install Rust from [rustup.rs](https://rustup.rs/) first and re-run; with no `cargo` present the installer stops with that explanation rather than leaving a file that only fails when you try to use it.

Verified continuously: the `Install scripts` workflow runs the supported matrix end to end, and separately asserts on Rocky 9 that the installer refuses and explains itself.

## Next steps

* [CLI Reference](cli-reference.md) — the daemon, harness wrappers, and self-update.
* [Configuration](configuration.md) — home directory, layered config, runtimes.
* [Architecture](architecture.md) — how it all fits together.
