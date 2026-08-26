# The TUI

`medulla` with no arguments starts the terminal app: a [ratatui](https://ratatui.rs/)
interface driven by `CloudRuntime`, which talks straight to the Medulla
backend over HTTP. There is no server to start and no socket to attach to. With
nobody signed in it opens a login screen; `medulla --mock` skips straight to
the offline demo runtime.

## The tabs

| Tab | What it is for |
| --- | --- |
| Overview | The live event feed, the active cycle and its results, the task ledger, any pending decision, and a This device panel for what this machine is hosting. |
| Sessions | The sessions running on this machine and the ones dispatched to it: a rail on the left, and the selected row's transcript, terminal, diff, or workflow run beside it. `Ctrl`+`]` attaches the live harness pane. |
| Workflows | Authored multi-step plans: a sidebar, a graph canvas, and a copilot that edits it. See [Workflows](../features/workflows.md). |
| Subconscious | A placeholder for the layer under the work: what it filters on intake, what it learns from the gap between expectation and outcome, and what it escalates for a human to approve. Nothing here is live yet. |
| Changes | The session's Git changes: a rail of changed files, commits, patches, and review comments, with the selected unified patch beside it. `b` sets the baseline. |
| Hosts | What capacity exists: Hosts, Harness Types, Hooks, Agent Templates, Add Host, Strategies. |
| Feedback | The feedback board for the active runtime, with the selected item's body and comments. A runtime with no board (the local and core runtimes, or a signed-out session) shows a single hint panel instead. |
| Settings | Usage, Appearance, Status line, Config, Feedback, Trace, Context, Account, Help, grouped under General, Debug, and About. |

`Tab` walks the top-level views. Within a tab, `↑↓` walk the left nav and `1`-`9`
jump to a page. The Settings tab's Help subpage, or `/help`, lists the
keybindings; `/usage`, `/config`, and `/theme` open the matching Settings pages.

The Workflows tab exists only in a build with the default `workflows` feature. A
slim build ships seven tabs instead of eight and no Workflows entry, because the
tab would have nothing to draw.

Three surfaces have render code in the crate but no entry in the tab bar of this
build: TokenMaxxxing (whose own sidebar pages are Overview, Bounties, and
Leaderboard), Tasks (which duplicates what the Sessions tab already shows per
lane), and Memory.

The persona-memory layer is out of this build. The `memory` module, its CLI, and
its config schema were removed along with the engine dependency they needed, so
there is no Memory tab and no `memory` config section.

## Hosting work on this device

A plain `medulla` is both the orchestrator that decides what work to hand out and
a host that runs it, with no configuration. See
[Configuration › Hosting on this device](configuration.md#hosting-on-this-device)
for the config keys, the environment overrides, and how the two halves are turned
off independently.

## Harness types

Hosts › Harness Types lists the OpenRouter-backed harness presets this install
has configured, above the credential status of each harness kind. Each row names
the preset, its base CLI, its model, and its host, and says whether the
environment variable that preset names is currently set. `a` adds a preset, `e`
edits the selected one, `d` deletes it, and `r` re-reads presets and detected
credentials.

Add and edit use one compact line,
`id | name | claude|codex|opencode|openhuman | model | fast-model | host-id`. The fields
that line has no room for (`apiKeyEnv`, `baseUrl`, `contextWindow`, `default`,
and the Codex overrides) are carried over from the existing entry, so editing
here never resets them; set them in the config file. Saving writes the file, but
the host that is already running keeps the presets it started with, so restart
`medulla` before work can run on a new preset. See
[Configuration › Custom harness presets](configuration.md#custom-harness-presets)
for the full shape.

## Adding another machine

Pairing needs one string to travel, the worker's address, and both halves are
copied in the direction that is easy.

1. On the orchestrator, open Hosts › Add Host and press `c`. That copies a
   single line which installs `medulla` if it is missing and starts the worker.
   Paste it into an SSH session on the machine you want to add.
2. The worker prints its address and hands it to **your** terminal's clipboard
   rather than the remote machine's, using OSC 52, so it survives the SSH
   boundary. Back on the orchestrator, press `a` and paste it, optionally
   followed by a label.

The clipboard step needs a terminal that accepts OSC 52. Most do, but tmux wants
`set -g set-clipboard on` and some terminals disable it for security. It is also
skipped when the daemon's output is piped rather than attached to a terminal.
Either way the address is printed on a line of its own, so you can select it by
hand.

To skip the copy entirely, name the worker: run `medulla daemon --handle
build-box` and type `@build-box` into Add Host. Pass `--no-pair` when the
daemon's output is being parsed by a script. The worker side is covered in full
under [`medulla daemon`](cli-reference.md#medulla-daemon).

## Declaring what there is to work on

A device that hosts a harness usually has more than one project on it. The
workspace registry lists every directory the fleet can work in: this machine's,
plus every other host's, which that machine declares.

What is registered for this device is exactly what reaches the orchestrator as
`capabilities.accessibleDirs`, alongside the harness's own summary of each
project. It is routing context, not a permission grant: a delegated task still
runs in `[host].workspace`. The list persists to `[host].workspaces` and is
advertised from the next launch.

The Workspaces page is not in the current build's Hosts nav; declaring an agent
is what puts work in a directory, and that is done from the host tree. Its draw
arm, keys, and `[host].workspaces` persistence all still build.

The registry is managed from the command line, where
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
the prompt), or cancel a task with `Alt`+`X`. The operation absorbs the change
and keeps running; it does not restart. `Ctrl`+`]` attaches the live harness pane
for the selected lane.

### What a harness row says it is doing

The glyph at the head of a harness row is the whole state in one character, and
three of the five move:

| Glyph | State | Animation |
| --- | --- | --- |
| `⠋` | a turn is in flight | spins |
| `●` | alive, idle at its composer | still |
| `⚠` | waiting on you | pulses in the attention colour |
| `✕` | failed, or exited non-zero | pulses red |
| `✓` | exited cleanly, or finished and held for you to read | still |

The spinner is the answer to a question the rail could not previously answer.
A harness thinking hard writes nothing, so a busy session and an idle one had
the same dot, the same liveness timestamp, and the same `busy` flag; the only
way to find out was to open the pane. Medulla now reads the harness's own
progress line and spins the row for exactly as long as the turn lasts.

The pulse is Medulla's own, counted off the render clock rather than delegated
to the terminal's blink attribute — most terminals, and every multiplexer,
ignore that attribute, so a cue that depended on it was invisible to most of the
people it was for. Its colour and rate are yours:

```toml
[theme]
attention = "yellow"
attentionBlink = true
attentionBlinkSeconds = 1.0   # one full bright→dim cycle; clamped to 0.2–10.0
```

Settings › Appearance edits all three live under **Attention cues**. A failure
pulses red regardless, so "it is asking you something" and "it broke" are never
the same colour.

### When a harness needs you

A harness stopped on its own permission prompt looks exactly like one that is
thinking hard: still running, still holding its session, saying nothing. Medulla
watches each harness's screen for that state and marks it — the row turns yellow
and pulses with a `⚠`, says what it is waiting for and for how long
("codex is asking permission · 42s"), and the Sessions tab carries a `⚠2` badge so
a stuck pane is visible from whatever tab you are on.

Most of it is recognised from what the harness paints, in order of specificity:

1. **Startup dialogs** — trust and permissions that gate the whole session.
2. **Named prompts** — distinctive phrases that each CLI writes when it is asking
   (e.g. `claude` shows "No, and tell Claude what to do differently"; `codex`
   shows "Allow Codex to…"). Claude's plan-mode exit menu is named as such, so
   the row says it finished planning rather than merely that it is asking.
3. **Numbered menus** — a caret resting on a numbered option, or `(y/n)`.
4. **Blocking errors** — a usage limit, an expired sign-in, a rejected
   credential. These are printed *instead of* a completed turn, so the work did
   not happen; the row says which, and the count includes it.
5. **The terminal bell** — the universal fallback, in case a prompt is worded
   differently or not recognised.

Two states cannot be read off a screen at all, and are taken from the session
itself. A harness that **died** leaves its terminal frozen on whatever it last
painted, which is often an ordinary composer — the row now goes red and says
what happened (`codex exited with 137`), where before it went quiet. And a
dispatched turn that **finished** leaves the session standing for you to read,
which looks identical to a session nobody has used; that row shows `✓ … finished
— read and release`. It is shown but not counted in the `⚠` badge: nothing is
held up while it waits, and a badge that ticks up on every successful task is a
badge you learn to ignore.

Two things clear the mark: attaching to the pane, and the orchestrator injecting
a prompt into that session. Both mean somebody is now dealing with it. A named
prompt comes straight back on the next sample if the harness is in fact still
asking, so nothing is hidden. A bell does not, because a ring has no second frame
to keep it alive.

## Read next

* [CLI Reference](cli-reference.md): every subcommand and flag.
* [Configuration](configuration.md): the Medulla home, layered config, runtimes.
* [Workers and Sessions](../features/workers-and-sessions.md): the model behind these screens.
