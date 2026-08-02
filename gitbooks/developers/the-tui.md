# The TUI

`medulla` with no arguments starts the terminal app: a [ratatui](https://ratatui.rs/)
interface over an OpenHuman core embedded in the same process. There is no
server to start and no socket to attach to. With nobody signed in it opens a
login screen; `medulla --mock` skips straight to the offline demo runtime.

## The tabs

| Tab | What it is for |
| --- | --- |
| **Overview** | The live event feed, the active cycle and its results, the task ledger, any pending decision, and a **This device** panel for what this machine is hosting. |
| **Agents** | Agent lanes and the chat composer together, with an attachable live harness pane. |
| **Workflows** | Authored multi-step plans: a sidebar, a graph canvas, and a copilot that edits it. See [Workflows](../features/workflows.md). |
| **TokenMaxxxing** | Token spend and headroom — **Overview**, **Bounties**, **Leaderboard**. |
| **Routing** | What capacity exists — **Hosts**, **Harnesses**, **Workspaces**, **Agent Templates**, **Add Host**, **Strategies**. |
| **Memory** | A placeholder. The persona-memory layer is out of this build, and the tab says so rather than disappearing. |
| **Settings** | **Usage**, **Appearance**, **Status line**, **Config**, **Trace**, **Context**, **Account**, **Help**, grouped under General, Debug, and About. |

`Tab` walks the top-level views. Within a tab, `↑↓` walk the left nav and `1`-`9`
jump to a page. The Settings tab's Help subpage — or `/help` — lists the
keybindings; `/usage`, `/config`, and `/theme` open the matching Settings pages.

The Workflows tab exists only in a build with the default `workflows` feature. A
slim build drops it rather than offering a tab that cannot draw anything.

## Hosting work on this device

A plain `medulla` is both halves of the system: the **orchestrator** that decides
what work to hand out, and a **host** that runs it. The host binds an address on
an in-process bus the orchestrator dispatches over, so a task for this machine is
delivered in memory — no tiny.place identity, no contact request, no relay
round-trip, and no second `medulla daemon` process beside the TUI. Workers on
other machines still travel over tiny.place, and the orchestrator picks per
address, so the two coexist with no configuration.

It is on by default and needs no setup. It serves whichever coding-agent CLIs it
finds on `PATH` (`claude`, `codex`, `opencode`), in the directory you launched
from. `MEDULLA_HOST=0` turns hosting off for one run; `MEDULLA_HUB=0` turns the
orchestrator half off. Set both and you have a plain chat client.

## Adding another machine

Pairing needs one string to travel — the worker's address — and both halves are
copied in the direction that is easy.

1. On the orchestrator, open **Routing › Add Host** and press `c`. That copies a
   single line which installs `medulla` if it is missing and starts the worker.
   Paste it into an SSH session on the machine you want to add.
2. The worker prints its address and hands it to **your** terminal's clipboard
   rather than the remote machine's, using OSC 52, so it survives the SSH
   boundary. Back on the orchestrator, press `a` and paste it, optionally
   followed by a label.

The clipboard step needs a terminal that accepts OSC 52 — most do, but tmux wants
`set -g set-clipboard on` and some terminals disable it for security. It is also
skipped when the daemon's output is piped rather than attached to a terminal.
Either way the address is printed on a line of its own, so you can select it by
hand.

To skip the copy entirely, name the worker: run `medulla daemon --handle
build-box` and type `@build-box` into Add Host. Pass `--no-pair` when the
daemon's output is being parsed by a script. The worker side is covered in full
under [`medulla daemon`](cli-reference.md#medulla-daemon).

## Declaring what there is to work on

A device that hosts a harness usually has more than one project on it.
**Routing › Workspaces** lists every directory the fleet can work in — this
machine's, which you add with `a` and remove with `d`, and every other host's,
which that machine declares and this page shows read-only.

What is listed here for this device is exactly what reaches the orchestrator as
`capabilities.accessibleDirs`, alongside the harness's own summary of each
project. It is routing context, not a permission grant: a delegated task still
runs in `[host].workspace`. The list persists to `[host].workspaces` and is
advertised from the next launch.

The same registry is reachable from the command line, where
[`medulla workspace add`](cli-reference.md#medulla-workspace) both drafts a
[`MEDULLA.md`](../features/workspace-profiles.md) profile and enrols the
directory:

```sh
medulla init                # write a MEDULLA.md and nothing else
medulla workspace add       # profile it and register it
medulla workspace list      # show the registry (--json for machines)
medulla workspace remove .  # unregister; files and MEDULLA.md are left alone
```

## Steering a running fleet

Sessions are not fire-and-forget. While a fleet is running you can correct the
plan, answer an agent's question by selecting its lane and typing (`Alt`+`A` for
the prompt), or cancel a task with `Alt`+`X`, and the operation absorbs the
change rather than restarting. `Ctrl`+`]` attaches the live harness pane for the
selected lane.

### When a harness needs you

A harness stopped on its own permission prompt looks exactly like one that is
thinking hard: still running, still holding its session, saying nothing. Medulla
watches each harness's screen for that state and marks it — the row turns yellow
and blinks with a `⚠`, says what it is waiting for and for how long
("codex is asking permission · 42s"), and the Agents tab carries a `⚠2` badge so
a stuck pane is visible from whatever tab you are on.

It is recognised from what the harness paints, in order of specificity:

1. **Startup dialogs** — trust and permissions that gate the whole session.
2. **Named prompts** — distinctive phrases that each CLI writes when it is asking
   (e.g. `claude` shows "No, and tell Claude what to do differently"; `codex`
   shows "Allow Codex to…").
3. **Numbered menus** — a caret resting on a numbered option, or `(y/n)`.
4. **The terminal bell** — the universal fallback, in case a prompt is worded
   differently or not recognised.

Two things clear the mark: attaching to the pane, and the orchestrator injecting
a prompt into that session — both mean somebody is now dealing with it. A named
prompt comes straight back on the next sample if the harness is in fact still
asking, so nothing is hidden; a bell does not, because a ring has no second frame
to keep it alive.

## Read next

* [CLI Reference](cli-reference.md) — every subcommand and flag.
* [Configuration](configuration.md) — the Medulla home, layered config, runtimes.
* [Workers and Sessions](../features/workers-and-sessions.md) — the model behind these screens.
