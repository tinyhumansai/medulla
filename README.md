![Medulla running seven agent sessions in one terminal](./docs/pitch.gif)

# Medulla

**One terminal. Every agent you have. Every machine you have.**

Medulla is a terminal for running many coding agents at once. Claude Code, Codex, OpenCode, a plain shell: each one gets its own live session in a rail down the side, on your laptop or on any machine you can SSH to, and you move between them with a keystroke. It replaces the pile of terminal windows, the tmux layout, and the ssh and mosh sessions you would otherwise keep open to do the same job.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex
```

Either script downloads the prebuilt binary for your platform, checks it against the release manifest, and installs it to `~/.medulla/bin` (`%USERPROFILE%\.medulla\bin` on Windows). If it changed your `PATH`, open a new terminal or run `exec $SHELL` so `medulla` resolves.

Then:

```sh
medulla login   # browser sign-in
medulla         # open the terminal
```

Not ready to sign in? `medulla --mock` runs an offline demo with no account and no network.

Prebuilt binaries ship for Linux (x86_64, aarch64), macOS (Apple Silicon), and Windows (x86_64). Building from source and pinning a version are covered in [Getting Started](https://tinyhumans.gitbook.io/medulla/developers/getting-started).

## What it does

Press `Ctrl-T`. Pick a machine (this one, or any remote host you have configured), pick an agent or a shell, pick a directory. The session is running. Press `Ctrl-T` again for the next one. There is no ceiling on how many you keep open, and each has its own terminal that keeps painting in the background whether or not it is the one on screen.

`Ctrl-]` attaches your keyboard to the selected session, and every key after that goes straight to the agent. `Ctrl-]` again gives the keyboard back. The other sessions keep running the whole time.

Medulla reads every session's screen for the moments that need a person: a permission prompt, a `(y/n)`, a startup dialog, a usage limit, a crash, a turn that finished and is waiting for you to read it. Each one becomes a `⚠` on its row and a count in the rail title, `⚠ 3 waiting on you`, so you find out without cycling through panes.

Remote machines work the same way. Add a host to your config, and the first time you open a session there Medulla runs your own `ssh` to start a daemon on the far side. After that handshake the connection is UDP, mosh-style, so it survives a closed laptop lid or a change of network without reconnecting. One connection carries every session on that machine, and the agent you started there runs with that machine's config, CLIs, and keys.

Press `d` on any session to see the Git diff of what it has changed since it launched, with line comments and inline edits. Press `k` to close it.

## Documentation

Full documentation lives at **[tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla)**.

Start with the product pages:

- [Sessions](https://tinyhumans.gitbook.io/medulla/features/sessions): the rail, attaching, and what a row tells you.
- [Remote machines](https://tinyhumans.gitbook.io/medulla/features/remote-hosts): adding a host and what runs on it.
- [Harnesses](https://tinyhumans.gitbook.io/medulla/features/harnesses): the agents Medulla can open and how they are launched.
- [Attention cues](https://tinyhumans.gitbook.io/medulla/features/attention): how a session says it needs you.
- [Workflows](https://tinyhumans.gitbook.io/medulla/features/workflows): saved multi-step plans whose steps run as sessions.

Running it yourself or building on it? Everything technical is in [Developers](https://tinyhumans.gitbook.io/medulla/developers): the TUI, the CLI, configuration and environment variables, authentication, the host link protocol, the Rust SDK, testing, and troubleshooting.

## Availability

Any registered account can use Medulla free for its first 30 days. After that it needs a Basic or Pro plan. See [Availability](https://tinyhumans.gitbook.io/medulla/availability).

## Open source

The client is the Rust workspace at [`tinyhumansai/medulla-src`](https://github.com/tinyhumansai/medulla-src): the [`medulla`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk) SDK crate and the [`medulla-tui`](https://github.com/tinyhumansai/medulla-src/tree/main/src/tui) crate that ships the binary, both under GPL-3.0-only. Read it, build it, and run the whole client offline against the mock runtime. Start with [Contributing](https://tinyhumans.gitbook.io/medulla/developers/contributing).

## What is in this repository

This is Medulla's documentation and distribution surface:

- `docs/`: engineering specs and protocol documents.
- `gitbooks/`: the sources behind [tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla).
- `install.sh` / `install.ps1`: the installers the commands above run.
- [Releases](https://github.com/tinyhumansai/medulla/releases): every published binary, its checksum, and the `latest.json` manifest `medulla update` reads.

The Rust workspace that produces those binaries is developed in `medulla-src`; its release pipeline publishes here.
