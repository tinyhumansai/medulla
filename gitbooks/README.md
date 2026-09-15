---
description: >-
  Medulla is a terminal for running many coding agents at once, on your own
  machine and on any machine you can SSH to, from one window.
cover: .gitbook/assets/screen.png
coverY: 356.141681768691
coverHeight: 417
layout:
  width: default
  cover:
    visible: true
    size: full
    mask: none
  title:
    visible: true
  description:
    visible: true
  tableOfContents:
    visible: true
  outline:
    visible: true
  pagination:
    visible: true
  metadata:
    visible: true
  tags:
    visible: true
  actions:
    visible: true
---

# Medulla

Medulla is one terminal for every coding agent you run. [Claude Code](https://www.anthropic.com/claude-code), [Codex](https://github.com/openai/codex), [OpenCode](https://github.com/sst/opencode), OpenHuman, and plain shells each get a live session in a rail down the side of the screen. They can be on your laptop or on any machine you can reach over SSH. You switch between them with a keystroke, and Medulla tells you which ones are waiting on you.

Before, running six agents across two machines meant six terminal windows, or a tmux layout, plus an ssh or mosh session for each remote box, and it meant you were the one cycling through panes to see which agent had stopped on a prompt. Medulla is one process in one window that does all of that.

## How it works

Press `Ctrl-T`. If you have remote hosts configured you pick a machine first; otherwise that step is skipped. Then pick an agent or a shell, then a directory. The session is running, on its own pseudo-terminal, with its own terminal emulator kept live in the background whether or not it is the one on screen. Selecting a different row shows a screen that is already current.

`Ctrl-]` hands your keyboard to the selected session. From then on every key goes to the agent, including `q`, `Esc`, `Tab`, and `Ctrl-C`; `Ctrl-]` is the only key Medulla keeps for itself, and pressing it again hands the keyboard back. Nothing pauses while you are attached elsewhere.

Medulla reads every session's screen for the moments that need a person. A permission prompt, a startup dialog, a numbered menu or a bare `(y/n)`, a usage limit or an expired sign-in, a terminal bell, a process that died, a turn that finished and is waiting to be read: each becomes a `⚠` on its row and a count in the rail title, `⚠ 3 waiting on you`. The mark clears when you attach.

A remote host is a `[[remoteHosts]]` entry in your config. The first time you open a session there, Medulla runs your own `ssh` to start a daemon on the far side and carry a key back. After that the two machines talk directly over UDP, mosh-style, so the link survives a closed laptop lid or a change of network. One connection carries every session on that host, and each session runs with that machine's own config, installed CLIs, and credentials.

## Where to go next

* [One terminal, not a pile of them](why-one-terminal.md): what a multiplexer and an ssh session cannot do, and what Medulla does instead.
* [Availability](availability.md): early alpha and how to request access.

The Features section covers what Medulla does day to day: [sessions](features/sessions.md), [remote machines](features/remote-hosts.md), [harnesses](features/harnesses.md), [attention cues](features/attention.md), and [workflows](features/workflows.md).

Running it yourself? The [Developers](developers/) section covers installing the [binary](developers/getting-started.md), the [TUI](developers/the-tui.md), the [CLI](developers/cli-reference.md), [configuration](developers/configuration.md), the [host link protocol](developers/host-link-protocol.md), and the [Rust SDK](developers/sdk.md).
