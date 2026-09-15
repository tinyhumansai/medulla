# Configuration

Medulla runs with no configuration at all: it finds the coding CLIs on your `PATH` and opens sessions in the directory you launched from. A config file is for remote hosts, custom model presets, hooks, and how the rail looks. The complete, commented shape of every section is in [`config.example.toml`](https://github.com/tinyhumansai/medulla-src/blob/main/config.example.toml) in the source repository; this page is the guided version.

## Medulla home

All config and data live under one directory:

| | Location |
| --- | --- |
| default | `~/.medulla` |
| `MEDULLA_DEV=1` | `./.medulla`, relative to the current directory (for local development) |
| `MEDULLA_HOME=<path>` | `<path>`, which beats both of the above |

Under the home:

```
<home>/credentials.json   saved by `medulla login` (mode 0600)
<home>/config.toml        the user-global config file
<home>/state              stateDir: chats, run records, control sockets
<home>/link               host-link identity
<home>/workflows          saved workflow documents
```

A `.env` file in the current directory is loaded at startup (`KEY=VALUE`, `#` comments, optional `export`, quotes stripped). It never overrides a variable already set in the environment. This is the usual way to set `MEDULLA_DEV=1` for local development.

## Layered config

Values combine from five layers, highest wins:

1. CLI flags (`--config`, `--no-alt-screen`, …)
2. Environment variables (`MEDULLA_API_URL`, `MEDULLA_TOKEN`, `MEDULLA_STATE_DIR`, the `MEDULLA_*` harness knobs; the older `TINYPLACE_*` spelling of those is still read)
3. The project-local file: `./.medulla/config.toml` or `./medulla.toml`
4. The user-global file: `<home>/config.toml`
5. Built-in defaults

Files merge field by field, so a project-local file can override just `backend.baseUrl` without discarding the rest of the global file. An explicit `--config <path>` (`.toml` or `.json`) replaces layers 3 and 4; environment variables and flags still override it.

Two sections are only ever read from the global file or an explicit `--config`, never from a project-local one: `[[remoteHosts]]` and `[[hooks]]`. Both can run a command on your machine (`sshOptions` accepts `-o ProxyCommand=…`; a hook is a command), so a config checked into a repository must not be able to supply them.

## Backend

```toml
[backend]
baseUrl = "https://api.tinyhumans.ai"   # MEDULLA_API_URL overrides; MEDULLA_STAGING=1 flips the default
tokenEnv = "MEDULLA_TOKEN"              # the env var holding a bearer JWT
# token = "eyJ..."                      # inline; discouraged
```

Token precedence is an inline `token`, then the variable `tokenEnv` names, then the session `medulla login` stored. See [Authentication](authentication.md).

### The mock runtime

`medulla --mock` runs a scripted, self-contained runtime with no backend and no login. It fabricates a plausible set of sessions and a feedback board so every tab has something to draw, and it is the fastest way to look at the interface. With nobody signed in, the login screen also offers it on `m`.

## Remote hosts

Other machines, reached over SSH. Each entry is one machine:

```toml
[[remoteHosts]]
host = "tower.local"                       # required: hostname, IP, or a ~/.ssh/config alias
name = "Tower"                             # display name; defaults to `host`
id = "tower"                               # stable id for sessions and the rail; defaults to a slug of name, then host
user = "steven"                            # omit to let ssh and ~/.ssh/config decide
port = 22                                  # 0 or omitted leaves ssh's default
identityFile = "~/.ssh/id_ed25519"
sshOptions = ["-o", "ProxyJump=bastion"]   # appended to the ssh argv verbatim
sshBinary = "ssh"                          # the ssh client to run
remoteCommand = "medulla"                  # how medulla is invoked on the far side
udpHost = ""                               # only when UDP cannot follow where SSH went
workspace = "/home/steven/src/repo"        # default directory for sessions there
workspaces = ["/home/steven/src/other"]    # also offered on the workspace step
enabled = true                             # false keeps the entry but stops offering it
```

SSH is only the bootstrap. It starts `medulla daemon --direct` on the far side and carries that daemon's key back inside the SSH channel, after which datagrams go straight to the machine over UDP, mosh-style. The connection is opened the first time you start a session there, not at startup, and one connection is shared by every session on that host.

`udpHost` is for the case where SSH reaches the machine through a jump host but UDP can go direct, or the reverse. `workspaces` is what the picker's workspace step can show, since a remote directory cannot be completed from here; anything you type is also accepted.

A session on a remote host gets whatever that machine is configured for: its `[router]`, its `[[hooks]]`, its `[attribution]`, its `[[customHarnesses]]`, and the CLIs installed there, all resolved from its own config file. Nothing is shipped from this one.

## Harness

How sessions you open by hand behave:

```toml
[harness]
skipPermissions = false                     # launch with the CLI's bypass flag
recentWorkspaces = ["/absolute/path/to/repo"]
favoriteWorkspaces = [{ name = "medulla", path = "/absolute/path/to/repo" }]
```

`skipPermissions` is off by default because a session you opened is attended, so there is someone to answer a prompt. `recentWorkspaces` is picker history, newest first, written by the TUI. `favoriteWorkspaces` are named shortcuts that stay useful after they fall out of the recent list.

## Custom harness presets

An OpenRouter model run through one of the CLIs. The CLI stays the agent; OpenRouter supplies the model.

```toml
[[customHarnesses]]
id = "deepseek"
name = "DeepSeek via Claude"
baseHarness = "claude"                 # claude | codex | opencode | openhuman
model = "deepseek/deepseek-chat"
fastModel = "deepseek/deepseek-chat"   # Claude's Sonnet/Haiku tiers, so sub-agents use the preset too
default = false
apiKeyEnv = "OPENROUTER_API_KEY"       # the variable's name; never the key
baseUrl = "https://openrouter.ai/api"  # optional; defaults per base harness
contextWindow = 114000                 # optional; Claude's auto-compaction window
providerOnly = ["streamlake"]          # optional; pin OpenRouter's serving provider
```

Presets are only offered when `OPENROUTER_API_KEY` (or whatever `apiKeyEnv` names) is set. The CLI never sees the real key: Medulla starts a loopback proxy, hands the CLI a machine-local token, and forwards to OpenRouter with Medulla's attribution headers. See [Attribution and routing](attribution-and-routing.md).

`providerOnly` matters because one model is offered by many providers at prices that differ by more than an order of magnitude, and OpenRouter's default weighs price against uptime rather than pinning one. The pin travels in the request body, so it is the only way to state it; a `:provider` suffix on the model id is accepted by OpenRouter and ignored.

`baseHarness = "openhuman"` runs the turn in-process on Medulla's own agent loop rather than spawning a CLI. The model resolves from `MEDULLA_OPENHUMAN_MODEL` or `MEDULLA_HARNESS_MODEL`, then the preset's `model`, then `[workflows] defaultModel`.

## Router

One OpenAI-compatible gateway for every harness, so inference is centralised and metered without editing each CLI's own config:

```toml
[router]
baseUrl = "https://router.example.com"
apiKeyEnv = "ROUTER_API_KEY"
models = { reasoning = "deepseek/deepseek-chat" }   # a missing entry falls through to the CLI's own default

[router.providers.claude]                           # per-provider override: claude / codex / opencode
baseUrl = "https://router.example.com/anthropic"
providerOnly = ["streamlake"]                       # OpenRouter-bound routes only
```

Absent entirely, which is the default, means no behaviour change.

## Theme

```toml
[theme]
primary = "red"                # selection highlight, panel titles, accents
accent = "magenta"             # overlay borders
selectionFg = "white"          # text on the selection background
dimBorder = "darkgray"         # panel borders
attention = "yellow"           # ⚠ rows, the tab badge, "N waiting on you"
attentionBlink = true          # false holds the cue steady
attentionBlinkSeconds = 1.0    # one full pulse; clamped to 0.2–10.0
```

Values are ratatui colour names (`cyan`, `lightblue`, `darkgray`, …) or `#rrggbb`. A failed session pulses red regardless of `attention`. Settings › Appearance edits these live and writes them back.

## Appearance

```toml
[appearance]
sidebarGrouping = "host"    # host | path | harness | none
sidebarSort = "created"     # created | recent | name
showSessionTitles = true
cpu = "off"                 # off | percent | value | bar   (this process)
ram = "off"
diskIo = "off"
deviceCpu = "off"           # whole-machine readings
deviceRam = "off"
deviceDisk = "off"
```

`host` grouping draws section headers only once a second host exists. Grouping only moves the headers; no row disappears whichever way these are set.

## Subscriptions

Usage meters drawn in the rail, one per paid allowance:

```toml
[subscriptions]
claudeSession = "bar"          # rolling five-hour window
claudeWeekly = "bar"           # seven-day window, all models
claudeScoped = "off"           # seven-day window for one model
claudeCredits = "off"
codexSession = "off"
codexWeekly = "bar"
openRouterCredits = "value"    # prepaid balance
tinyhumansBalance = "off"
refreshSeconds = 300           # floor of 60
openRouterKeyEnv = "OPENROUTER_API_KEY"
```

Each meter is `off`, `percent`, `bar`, or (for balances) `value`. `off` means not sampled either. Claude meters read claude.ai with the local Claude CLI's own login; Codex meters read the newest Codex transcript on this machine; OpenRouter reads its credits endpoint with the key `openRouterKeyEnv` names. Settings › Subscriptions edits these live and shows each reading beside its setting.

## Status line

How a session row is laid out. Every field answers the same three questions: which line (`line1`, `line2`, `line3`, `hidden`), when (`always`, `active`, `alert`), and how it is spelled.

```toml
[statusLine]
state = "line1"
stateWhen = "always"
harness = "line1"
harnessWhen = "always"
harnessStyle = "short"        # long (Claude Code) | short (claude) | icon
control = "line1"             # managed / unmanaged: a workflow's session or yours
controlWhen = "always"
controlStyle = "text"         # text | icon
thread = "line2"
threadWhen = "always"
branch = "line1"
branchWhen = "always"
worktree = "line1"            # the linked worktree's name, when there is one
worktreeWhen = "always"
path = "line1"
pathWhen = "always"
pathStyle = "shortened"       # full | shortened (~/…/tail) | last
```

The defaults produce an identifying line plus the thread beneath it:

```
● codex · unmanaged · main · ~/work/medulla
  Ship the status line
```

Settings › Status line edits these live with a preview, which is the easier way to arrive at a layout.

## Attribution

```toml
[attribution]
commit = true
```

A `Co-authored-by: Medulla` trailer on the commit message of any commit a Medulla-launched harness makes. Message only, never the author or committer, so blame and `git log --author` are unaffected.

## Hooks

Medulla installs lifecycle hooks into every harness it launches. Its own built-ins only report back (they never deny a tool call or rewrite an input); `[hookDefaults] enabled = false` turns them off. Your own hooks are declared once here and reach Claude Code (through `--settings`) and Codex (through a `-c` override) from the same lines:

```toml
[hookDefaults]
enabled = true

[[hooks]]
event = "PostToolUse"          # PreToolUse · PostToolUse · PermissionRequest · UserPromptSubmit · Stop
matcher = "Edit|Write"         #   SubagentStart · SubagentStop · PreCompact · PostCompact
type = "command"               #   SessionStart · SessionEnd · Notification (Claude Code only)
command = "just fmt"

[[hooks]]
event = "sessionStart"         # camelCase is accepted too
type = "command"
command = "git fetch --all --prune"
timeout = 30
harnesses = ["claude"]         # lowercase; default is every harness that supports the event
```

The command runs through your shell, gets the harness's native hook payload on stdin, and speaks the harness's native decision protocol on stdout. A hook on an event a harness does not implement is dropped for that harness with a log line saying so. An unknown event name fails the whole config load, because a hook you believe is installed and is not is worse than a startup that stops and tells you. Your own `~/.claude/settings.json` hooks keep running as they do outside Medulla.

## Update

```toml
[update]
check = true
```

The TUI checks GitHub for a newer release about ten seconds after startup and every six hours, and shows a banner. `MEDULLA_NO_UPDATE_CHECK=1` also turns it off; `MEDULLA_UPDATE_URL` overrides the manifest. See [`medulla update`](cli-reference.md#medulla-update).

## State directory

```toml
stateDir = "/absolute/path/to/state"   # default <home>/state; MEDULLA_STATE_DIR overrides
```

## Workflows

Only read in a build with the `workflows` feature. Definitions live under `<home>/workflows`, run state under `<home>/state/workflows`.

```toml
[workflows]
enabled = true
defaultProvider = "claude"          # harness hint for agent steps that name none
defaultModel = "claude-sonnet-4-5"
allowCode = true                    # script nodes run unsandboxed; set false before loading workflows you do not trust
shell = "user"                      # interpreter for shell nodes; "user" follows $SHELL
shellArgs = ["-l"]
runTimeoutSecs = 600
toolAllowlist = []
httpAllowlist = []                  # HTTP is deny-by-default
maxParallelAgents = 4               # clamps a graph asking for more
maxLoopIterations = 25
maxListedRuns = 15                  # only the listing; every run stays in the store

[workflows.evolve]
enabled = true
autoOnFailure = true                # a failed run starts a review; the one worth turning off
maxRuns = 5
maxNotes = 40
```

## MCP

The tool server Medulla offers each harness it spawns, as `medulla mcp` in a subprocess with a socket path and grant token minted per session:

```toml
[mcp]
fleetTools = true                   # offer fleet_* alongside workflow_*
maxDepth = 2                        # how deep a dispatch tree may go; 0 lets no spawned harness dispatch
maxInFlight = 4                     # absent follows workflows.maxParallelAgents
socketPath = "/absolute/path/to/control.sock"   # absent uses an account-scoped path under the home
```

Nothing here writes to another program's config, and nothing it enables is reachable by a process Medulla did not spawn.

## Link

The host link identity. The pair key is deliberately absent from config: it is generated per connection and lives only in `<stateDir>/node.json`, never in a file that gets copied and pasted into issues.

```toml
[link]
forwarderUrl = "https://api.tinyhumans.ai"   # defaults to backend.baseUrl
nodeName = "my-laptop"
stateDir = "/absolute/path/to/link"          # default <home>/link
peers = []                                   # enrolled forwarder peers, if any
```

## Sections you can leave alone

`config.example.toml` also documents `[host]`, `[[hosts]]`, `[budget]`, `[core]`, `[workflow]` (singular), `[opencode]`, `[medulla]`, and the `[[fleet.*]]` tables. Those describe capacity for a dispatching model that is no longer part of the product. They are still parsed so an older file loads, but a session you open from the picker does not read them.

## Read next

* [Environment variables](environment-variables.md): every `MEDULLA_*` variable.
* [Remote machines](../features/remote-hosts.md): the user's view of `[[remoteHosts]]`.
* [Attribution and routing](attribution-and-routing.md): the loopback proxy behind presets and `[router]`.
