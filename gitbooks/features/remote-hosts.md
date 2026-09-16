---
description: >-
  A remote host is a machine you can SSH to. Add it once, and its agents and
  shells show up in the same rail as the ones on your laptop.
---

# Remote machines

Sessions on another machine work the same way as sessions on this one. You pick the host on the first step of `Ctrl-T`, then the harness, then a directory there, and the row joins the rail under that host's name. Attaching, the attention cues, the diff view, and closing all behave the same.

What you never do is hold an ssh window open, or run tmux on the far side. Medulla uses ssh once, to get started, and then talks to the machine directly.

## Adding a host

A host is a `[[remoteHosts]]` entry in your global config, `~/.medulla/config.toml`:

```toml
[[remoteHosts]]
host = "tower.local"                       # hostname, IP, or a ~/.ssh/config alias
name = "Tower"                             # what the rail calls it (default: host)
user = "steven"                            # omit to let ssh decide
workspace = "/home/steven/src/repo"        # default directory for sessions there
workspaces = ["/home/steven/src/other"]    # more directories the picker can offer
```

`host` is the only required field. The rest, and the less common ones (`port`, `identityFile`, `sshOptions`, `sshBinary`, `remoteCommand`, `udpHost`, `enabled`), are covered in [Configuration › Remote hosts](../developers/configuration.md#remote-hosts).

Medulla must be installed on the far side. If it is not, the first connection fails with a message that says so and gives the install command.

This section is only read from your global config. A `[[remoteHosts]]` block in a project-local `medulla.toml` is ignored, because `sshOptions` can run a command on your machine and a checked-in config should not be able to do that.

## What happens when you open a session there

Nothing happens at startup. A configured host costs nothing until you pick it, so a host that is switched off, or on a network you are not on, does not slow anything down.

The first time you pick a host, Medulla runs your own `ssh` client, with your `~/.ssh/config`, your keys, your `known_hosts`, and any `ProxyJump` you have set up. It starts `medulla daemon --direct` on the far side. That daemon mints a key for this connection, prints one line carrying its UDP port, node id, and the key, and detaches; ssh then exits.

From then on the two machines talk over UDP with that key. The transport is mosh-style state synchronisation rather than a byte stream, so a closed laptop lid or a change of Wi-Fi network does not drop the session or need a reconnect. One connection carries every session on that host; only the bootstrap is per host, and it happens once.

A host's row in the rail shows where it is: idle, connecting, live, or failed with the reason. The useful failures are an unknown host key, a missing `medulla` binary, and a refused login, and each says which.

## What the remote session gets

Whatever that machine is configured for. Its own config file decides its router, its hooks, its custom harness presets, and its commit attribution, and the coding CLIs it offers are the ones actually installed there. Nothing is shipped from your laptop. A preset that exists only on the build box can be opened from a laptop that has never heard of it, and a laptop cannot make the build box run a hook command it did not agree to.

The remote daemon binds its own control socket, so an agent it starts gets its tools, its managed skills, and somewhere to report, exactly as a local one does.

## Watching a remote screen

Every remote session's presence and state ride the reliable channel, so the rail always knows what is running there and which rows are waiting. The screen itself streams for one session per host at a time, the one you have selected; the transport carries a single synchronised grid per peer, so that is a property of the link rather than a limit Medulla chose.

The "waiting for 42s" reading on a remote row is measured from when your machine first saw the cue, not from the remote clock, so a host with skewed time does not show a fresh prompt as stuck for hours.

## One command, no session

`medulla remote <host> --exec "<command>"` runs a command on a host and prints what its screen showed, using the same bootstrap and link:

```sh
medulla remote tower --exec "git status"
medulla remote tower --harness claude     # open a session there and print what it says
```

It is the smallest thing that exercises the whole chain, which is why the test suite is built on it, and it answers "what does the build box say" without opening the TUI.

## Read next

* [Configuration › Remote hosts](../developers/configuration.md#remote-hosts) for every field.
* [Host link protocol](../developers/host-link-protocol.md) for the wire format.
* [Troubleshooting](../developers/troubleshooting.md) for connection failures.
