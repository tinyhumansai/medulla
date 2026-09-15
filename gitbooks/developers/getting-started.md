# Getting Started

Medulla ships as a single binary, `medulla`: a [ratatui](https://ratatui.rs/) terminal app that runs your coding agents in sessions, on this machine and on any you can SSH to, and tells you which ones need you.

## Prerequisites

* A real terminal. The TUI refuses to start without a TTY.
* At least one coding CLI on your `PATH`: [Claude Code](https://www.anthropic.com/claude-code), [Codex](https://github.com/openai/codex), or [OpenCode](https://github.com/sst/opencode). Without one the picker still offers your shells.
* To build from source: Rust stable (edition 2021) via [rustup](https://rustup.rs/).

## Install the prebuilt binary

Both installers do the same thing: resolve the release manifest, download the asset for your platform, verify its SHA-256, and install to `~/.medulla/bin`. If no prebuilt asset ships for your platform, they fall back to `cargo install`.

### macOS and Linux

`install.sh` is POSIX `sh` and needs `curl` or `wget` plus `tar`:

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh -s -- 0.5.3   # pin a version
```

The checksum is verified when `sha256sum`, `shasum`, or `openssl` is available. With none of them it warns and skips the check instead of failing.

### Windows

`install.ps1` works in PowerShell 7 (`pwsh`) and in the Windows PowerShell 5.1 that ships in the box:

```powershell
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

It installs to `%USERPROFILE%\.medulla\bin` and appends it to your user `PATH`, so no elevation is needed. Verification is always on, since `Get-FileHash` is built in. To pin a version, the script needs to be on disk so it can take an argument:

```powershell
iwr -useb https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.5.3
```

If a running `medulla.exe` holds a lock on its own image, the installer moves it aside as `medulla.exe.old` instead of failing.

### Installer environment variables

Both scripts honour the same environment variables:

| Variable | Effect |
| --- | --- |
| `MEDULLA_HOME` | Install prefix (default `~/.medulla`). |
| `MEDULLA_NO_MODIFY_PATH=1` | Do not touch shell profiles or the user `PATH`. |
| `MEDULLA_UPDATE_URL` | Override the release manifest URL (for testing). |

If the installer updated your `PATH`, open a new terminal (or run `exec $SHELL`) so the `medulla` command resolves; otherwise invoke it directly from the install prefix.

Prebuilt binaries ship for Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64). See [platform support](#platform-support) for what is unix-only. Every installer is exercised on each of those platforms, plus Fedora, Debian, and Rocky, by the `Install scripts` CI workflow, which installs from the real published release and then runs the binary.

## Build from source

```sh
git clone https://github.com/tinyhumansai/medulla-src
cd medulla-src
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

`medulla login` opens your browser, captures the JWT the backend redirects back with, verifies it, and saves credentials under your [Medulla home](configuration.md#medulla-home). The next `medulla` run picks them up automatically. Over SSH, where the browser cannot reach back, use `medulla login --code`. Providers, headless tokens, and the security model are covered in [Authentication](authentication.md).

If you start `medulla` with no working credentials, the TUI shows an in-terminal login screen with the browser flow and a paste-a-token option. It does not offer the mock runtime; that is `medulla --mock`, so a failed sign-in cannot quietly land you in a demo.

Once in, press `Ctrl-T`, pick a harness, pick a directory, and the session is running. `Ctrl-]` attaches your keyboard to it and detaches again. [The TUI](the-tui.md) has every key.

## Add a remote machine

Install `medulla` on the other machine the same way, then add it to `~/.medulla/config.toml` on this one:

```toml
[[remoteHosts]]
host = "tower.local"
workspace = "/home/you/src/repo"
```

The next `Ctrl-T` starts with a host step. Picking the new host runs your own `ssh` once to start a daemon there; after that the two machines talk over UDP directly. See [Remote machines](../features/remote-hosts.md).

## Explore with zero setup

```sh
cargo run -- --mock     # or: medulla --mock
```

`medulla --mock` runs the mock runtime, a scripted demo with no credentials and no network, and the fastest way to explore the interface. Open the Settings tab and its Help subpage for the keys. Usage, the effective config, and the theme editor live under Settings as well.

## Use the SDK from your own crate

Add the SDK as a git dependency (the repo vendors its path deps, so no extra setup):

```toml
[dependencies]
medulla = { git = "https://github.com/tinyhumansai/medulla-src", tag = "v0.3.0" }
```

The [`medulla` SDK crate](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/) is a UI-free logic library: config, auth, the backend client, harness detection and launch, the clipboard writers, the inference proxy, workflows, and the daemon. See [Architecture](architecture.md) for how the pieces fit together.

## Platform support

Linux (x86\_64, aarch64), macOS (Apple Silicon), and Windows (x86\_64) all build and ship prebuilt binaries. The [daemon's](cli-reference.md#medulla-daemon) provider-spawn paths and the [harness wrappers](cli-reference.md#harness-wrappers) are unix-only, which also means a remote host must be a unix machine. The interactive TUI, `medulla login`, and `medulla update` work everywhere.

### The Linux glibc floor

The prebuilt Linux binaries are built on Ubuntu 24.04 and therefore need glibc 2.39 or newer. That covers Ubuntu 24.04+, Debian 13+, and current Fedora, but *not* the long-term enterprise distributions: RHEL 9 and its rebuilds (Rocky, AlmaLinux), Debian 12, or Amazon Linux 2023, which ship glibc 2.34-2.36.

On those systems the download would succeed and every later invocation would die on a missing symbol version, so `install.sh` runs the binary once immediately after installing it. If it cannot start, the installer removes it, says why, and falls back to building from source, which works anywhere Rust does. Install Rust from [rustup.rs](https://rustup.rs/) first and re-run. With no `cargo` present the installer stops and explains, rather than leaving a file that only fails when you try to use it.

The `Install scripts` workflow runs the supported matrix end to end on every change, and separately asserts on Rocky 9 that the installer refuses and explains itself.

## Read next

* [CLI Reference](cli-reference.md): the daemon, `remote`, and self-update.
* [Configuration](configuration.md): home directory, layered config, runtimes.
* [Architecture](architecture.md): how it all fits together.
