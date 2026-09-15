# Glossary

The words Medulla uses, and what each one means here. Other tools use several of them loosely; on this page they are precise.

## Session

One running process on its own pseudo-terminal: a coding agent in a directory, or a shell. A session has a row in the rail, a screen kept live in the background, and a state (in flight, idle, waiting on you, failed, finished). Sessions exist on this machine and on remote hosts, and the rail shows both. See [Sessions](../features/sessions.md).

## Harness

The program a session runs. Claude Code, Codex, OpenCode, OpenHuman, or a shell. The word is borrowed from the agent world, where a harness is the CLI around a model, and Medulla keeps it because the CLI, not the model, is what is launched. See [Harnesses](../features/harnesses.md).

## Provider

The wire name for a harness kind: `claude`, `codex`, `opencode`, `openhuman`, `shell`. What `MEDULLA_<PROVIDER>_BIN` and `baseHarness` name.

## Custom harness preset

A `[[customHarnesses]]` entry: an OpenRouter model run through one of the CLIs, with the key kept behind a loopback proxy. Appears in the picker by name.

## Host

A machine sessions can run on. This one, or a remote host.

## Remote host

A `[[remoteHosts]]` entry: another machine, reached over your own `ssh` for the bootstrap and then directly over UDP. Its sessions run with its own config and CLIs. See [Remote machines](../features/remote-hosts.md).

## Workspace

The directory a session starts in. The third step of the picker.

## Attach and detach

Attaching hands your keyboard to a session; every key goes to its process. Detaching gives the keyboard back to the rail. `Ctrl-]` does both. The pane title says which state you are in.

## Focus chord

`Ctrl-]`, the one key Medulla keeps for itself while you are attached. Terminals that cannot send it report `Ctrl-5`, which is accepted as the same chord.

## Attention cue

A signal that a session needs a person: a permission prompt, a startup dialog, a numbered menu, a `(y/n)`, a usage limit, a bell, an exit, or a finished dispatched task. A cue puts a `⚠` on the row, a count in the rail title, and a badge on the tab. See [Attention cues](../features/attention.md).

## Rail

The list of sessions down the left of the Sessions tab. Grouped by host, path, harness, or nothing.

## Status line

The one-to-three-line layout of a session row: state glyph, harness, control, thread, branch, worktree, path. Set under `[statusLine]` or Settings › Status line.

## Daemon

`medulla daemon`, the process that serves sessions on a machine to a Medulla elsewhere. `medulla daemon --direct` is the variant the SSH bootstrap starts on a remote host: one client, one pair key, no forwarder.

## Connect line

The single line `medulla daemon --direct` prints to stdout before detaching: `MEDULLA-CONNECT v1 port=… node=… key=…`. The client reads it from the SSH channel and never needs SSH again for that host.

## Host link

The transport between a Medulla and a remote host: `medulla-link/1`, UDP datagrams carrying mosh-style state synchronisation under a pair key. Not a stream, which is why it survives a closed lid. See [Host link protocol](host-link-protocol.md).

## Pair key

The 128-bit key the two ends of a host link share. Minted per connection, carried once inside the SSH channel, stored only in `<stateDir>/node.json`, and never in config.

## Node id

A link endpoint's identity, 16 bytes, hex on the wire. The client passes its own as `--peer-node` so the daemon knows whom to serve.

## Forwarder

A backend relay that forwards host-link datagrams it cannot read, for enrolled peers that do not share a network. The SSH-bootstrapped case does not use one.

## Medulla home

`~/.medulla` by default, or `MEDULLA_HOME`, or `./.medulla` under `MEDULLA_DEV=1`. Holds credentials, config, state, the link identity, and saved workflows.

## Mock runtime

The scripted offline runtime behind `medulla --mock` and the `m` key on the login screen. No account, no network, every tab has something to draw.

## Hook

A command Medulla installs into a launched harness's lifecycle: its own reporting hooks (`[hookDefaults]`) and yours (`[[hooks]]`). Reaches Claude Code and Codex from the same declaration.

## MCP grant

The per-session token Medulla mints when it hands a harness the `medulla mcp` tool server. What the holder may do is looked up from the grant server-side; the harness's own claims about itself count for nothing.

## Workflow

A saved multi-step plan, a directed acyclic graph whose `agent` steps each open a harness session, with parallel branches and approval gates. Optional; behind the `workflows` feature. See [Workflows](../features/workflows.md).

## Skill

A harness-native file (`medulla skills install`) that lets an everyday Claude Code or Codex session started outside Medulla find and run a saved workflow.

## ACP (Agent Client Protocol)

The protocol Medulla uses as a client to drive Claude Code and Codex headlessly, in workflow steps and the daemon. Because Medulla is the client, the only way to hand a harness tools is to offer it an MCP server, which is what `medulla mcp` is for. See [Harness integration](harness-integration.md).

## Read next

* [Sessions](../features/sessions.md) and [Remote machines](../features/remote-hosts.md) for the vocabulary in use.
* [Architecture](architecture.md) for where each of these lives in the code.
