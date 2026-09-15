# The TUI

`medulla` with no arguments starts the terminal app: a [ratatui](https://ratatui.rs/) interface that owns a set of pseudo-terminals, one per session, and draws whichever you have selected. There is no server to start. With nobody signed in it opens a login screen; `medulla --mock` skips straight to the offline demo runtime.

## The tabs

| Tab | What it is for |
| --- | --- |
| Sessions | Every session you have open, local and remote, as a rail on the left, with the selected one's terminal beside it. This is where you spend your time. |
| Workflows | Saved multi-step plans: a sidebar, a graph canvas, and a copilot that edits the graph. Present only in a build with the default `workflows` feature. See [Workflows](../features/workflows.md). |
| Subconscious | A placeholder for a layer under the sessions: what gets filtered on the way in and what is escalated for a person to approve. It draws nothing live yet, and is listed so you can see where approvals will surface. `E` reviews prepared decisions. |
| Feedback | The feedback board for the signed-in account, with the selected item's body and comments. The mock runtime shows a scripted board; a signed-out session shows a hint panel. |
| Settings | Usage, Appearance, Subscriptions, Status line, Config, Feedback, Trace, Context, Account, and Help, grouped under General, Debug, and About. |

`Tab` and `Shift-Tab` walk the tabs. `Ctrl-C` quits.

There is no chat composer. Medulla is not something you type at; you type into the sessions it holds.

## The Sessions tab

The rail lists sessions. `↑↓` walk it (`Alt-↑`/`Alt-↓` for terminals that eat the plain arrows), `Enter` on a row attaches to it, and `Enter` on the `+ New session` row opens the picker. Rows are grouped by host, workspace path, harness, or nothing, and sorted by creation, recency, or name, per `[appearance]` in the config or Settings › Appearance.

### Opening a session

`Ctrl-T` opens the picker from anywhere. It has three steps, and the first is skipped when there is only one host:

1. **Host.** This machine, or a `[[remoteHosts]]` entry. Picking a remote host that is not yet connected runs the SSH bootstrap; the picker waits on this step until the host answers, because the harness list has to come from the host.
2. **Harness.** The CLIs found on that host's `PATH`, then OpenHuman if installed, then custom presets, then shells. The first row is what `Enter` starts, so a coding agent leads and shells are the exception you scroll to.
3. **Workspace.** Type to filter, `Tab` completes, `Enter` starts, `Esc` goes back. Recent directories, favourites from `[harness] favoriteWorkspaces`, and folders under the current directory are offered. A pasted path lands in the filter.

The new row is selected as soon as it exists, so the pane shows it straight away.

### Attaching and detaching

Attaching hands the keyboard to the session. It is explicit and modal, and the pane title says which of the two owns the keyboard, because an embedded terminal where it is ambiguous is one where `q` quits the wrong thing.

`Ctrl-]` is the focus chord: it attaches when you are on the rail and detaches when you are in a session. While attached it is the only key Medulla keeps; everything else, including `Tab`, `Esc`, and `Ctrl-C`, is encoded and written to the session's PTY. Some terminals cannot send `Ctrl-]` and report it as `Ctrl-5` instead, which Medulla accepts as the same chord.

Background sessions keep painting. They just do not receive input.

### The diff pane

`d` on a session row swaps the terminal for the Git diff of what that session has changed since it launched. `↑↓` select files, `j`/`k` move by line, `[` and `]` jump hunks, `PageUp`/`PageDown` move faster. `c` comments on a line or hunk, `e` edits it, `C` comments on or edits the file, `r` refreshes. `d` or `Esc` puts the terminal back.

### Closing

`k` closes the selected session, after asking. `K` then `y` kills the underlying process; any other key after `K` cancels. `⌥X` cancels a dispatched task and `⌥A` answers its open question, for sessions that a workflow run started.

### What a row says

The glyph at the head of a row is its state: `⠋` spinning while a turn is in flight, `●` alive and idle, `⚠` waiting on you, `✕` failed or exited non-zero, `✓` finished a dispatched task and holding the result. A waiting row adds a line saying what for and for how long. The rail title carries the count, `⚠ 3 waiting on you`, and the tab bar carries a badge. [Attention cues](../features/attention.md) has the full list of what Medulla watches for and what clears a mark.

The rest of the row is the status line, whose fields and positions are set under Settings › Status line or `[statusLine]`.

## Remote hosts

A `[[remoteHosts]]` entry becomes a group in the rail. Its status shows there: idle, connecting, live, or failed with the reason. Sessions on it are rows like any other, and their state, cues, and presence ride the reliable channel; the screen streams for the selected one. [Remote machines](../features/remote-hosts.md) covers the bootstrap and what the far side gets.

## Settings

`↑↓` move between subpages and `1`-`9` jump to one. Inside a subpage, `j`/`k` pick a row and `←/→` or `Enter` change it; Appearance, Subscriptions, and Status line save live and draw a preview beside the list. Config edits the loaded `config.toml` in place. Feedback has `u`/`d` to vote, `c` to comment, `n` and `b` to file a feature or a bug, `s` and `f` to sort and filter. Trace and Context (under Debug) page events and chunks with `j`/`k`. Account logs out with `Enter` twice. Usage refreshes with `r`. Help lists the keys.

## Mouse

Click a tab to switch, click a rail row to select it, and the wheel scrolls. `Ctrl-O` releases the mouse to the terminal so native drag-select works; press it again to take it back.

## Other keys

| Key | Effect |
| --- | --- |
| `Ctrl-N` | New thread. |
| `Ctrl-X` | Abort: the copilot turn on the Workflows tab if one is running, otherwise the runtime. |
| `Ctrl-Up` / `Ctrl-Down` | Walk open threads on the Sessions tab. |

## Read next

* [CLI Reference](cli-reference.md): every subcommand and flag.
* [Configuration](configuration.md): the Medulla home, layered config, and every section.
* [Sessions](../features/sessions.md): the same surface from the user's side.
