---
description: >-
  A session is one agent or shell running on its own terminal. The rail lists
  every one you have open, on every machine, and tells you which need you.
---

# Sessions

A session is one running process on its own pseudo-terminal: Claude Code in a repo, Codex in another, a shell on the build box. The Sessions tab is a rail of every session you have open down the left and the selected session's screen beside it. That screen is live for every row, not just the one you are looking at, because each session has its own terminal emulator that keeps painting in the background. Switching rows shows a screen that is already current.

Nothing caps how many you keep open.

## Opening one

`Ctrl-T` opens the picker, or press `Enter` on the `+ New session` row. It has up to three steps:

1. **Host.** Which machine the session runs on: this one, or any `[[remoteHosts]]` entry in your config. This step is skipped when there is only one host, so with no remote machines configured you never see it.
2. **Harness.** The coding CLIs found on that machine's `PATH` (Claude Code, Codex, OpenCode), OpenHuman, any custom presets you have registered, and then the shells installed there. Your own `$SHELL` comes first. See [Harnesses](harnesses.md).
3. **Workspace.** The directory to start in. Type to filter, `Tab` completes, `Enter` starts. Recent directories and saved favourites are offered first. On a remote host the list is whatever that host's config declares plus anything you type, since a remote path cannot be completed from here.

A shell session is an ordinary terminal in the same pane. It attaches and detaches like any other session and nothing else ever types into it.

## Attaching

`Enter` on a session row, or `Ctrl-]`, attaches your keyboard to it. While attached, every key goes to the process: `q`, `Esc`, `Tab`, `Ctrl-C`, arrow keys, all of it. The one key Medulla keeps is `Ctrl-]`, which detaches and puts the keyboard back on the rail. (Terminals that cannot send `Ctrl-]` can use `Ctrl-5`, which is the same byte.)

Every other session keeps running while you are attached to one. Attaching also clears that session's attention mark, since someone is now dealing with it.

## Reading the rail

Each row is a session. The glyph at its head is its state:

| Glyph | State |
| --- | --- |
| `⠋` | a turn is in flight (spins) |
| `●` | alive and idle at its composer |
| `⚠` | waiting on you (pulses in the attention colour) |
| `✕` | failed, or exited non-zero (pulses red) |
| `✓` | finished a dispatched task and is holding the result for you to read |

A row that is waiting adds a line saying what for and for how long, such as `codex is asking permission · 42s`. The rail's title carries the total: `Sessions · 12 · 9 running · ⚠ 3 waiting on you`, and the tab bar carries a `⚠2` badge so you see it from any tab. [Attention cues](attention.md) lists everything Medulla watches for.

The rest of the row is the status line. Which fields it shows (state, harness, who has control, thread, Git branch, worktree, path) and where each sits is set under Settings › Status line or in `[statusLine]` in the config. Rows are grouped by host, workspace path, harness, or not at all, and sorted by creation, recency, or name, under Settings › Appearance or `[appearance]`.

`↑↓` walk the rail. `Alt-↑` and `Alt-↓` do the same, for terminals that eat the plain arrows. Clicking a row selects it and the wheel scrolls.

## Seeing what a session changed

`d` on a session row swaps the terminal for a Git diff of everything that session has changed since it launched. `↑↓` move between files, `j`/`k` move by line, `[` and `]` jump between hunks, `PageUp`/`PageDown` move faster. `c` leaves a comment on a line or hunk, `e` edits it in place, `C` comments on or edits the whole file, and `r` refreshes. `d` or `Esc` puts the terminal back.

## Closing one

`k` closes the selected session, after asking. `K` then `y` kills the underlying process without ceremony; any other key after `K` cancels. A session that has exited leaves the rail on its own; it is not kept as a row.

Sessions on remote hosts behave the same way. The row lives under that host's group in the rail, and closing it reaches across the link to the process on the far side.

## What survives a restart

The agent CLIs keep their own transcripts on disk, and those are what a resume reads. `medulla sessions` lists the recent Claude Code and Codex sessions found on this machine as JSON, read from the harnesses' own directories, and each resolves to something resumable in its original working directory.

Scrollback is 2,000 lines per session.

## Copying out

`Ctrl-O` releases the mouse to the terminal so you can drag-select and copy natively; press it again to take the mouse back. When Medulla copies something itself, such as a connect line, it writes to the terminal you are looking at using OSC 52, so the text lands on your laptop's clipboard even when Medulla is running inside ssh or tmux. [Troubleshooting › Copying out of Medulla](../developers/troubleshooting.md#copying-out-of-medulla) covers the nested cases.

## Read next

* [Remote machines](remote-hosts.md) for sessions on other hosts.
* [Harnesses](harnesses.md) for what the picker offers and how each is launched.
* [Attention cues](attention.md) for what makes a row light up.
