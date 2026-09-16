# Hosts

Medulla can drive several machines from one terminal. A **host** is another
machine you have paired once; after that the TUI reaches it directly over UDP,
mosh-style — no SSH session to keep alive, no relay in between, and a laptop
that sleeps or changes networks picks up where it left off.

The machine you pair must be Unix (Linux or macOS): the daemon's provider-spawn
paths and the harness wrappers it runs are unix-only, the same requirement as
[remote machines](remote-hosts.md) generally. Windows can run the Medulla TUI
that *dials in*, just not be the paired host on the other end.

## Pairing a machine

1. On the **Hosts** tab press `a`. Give the machine a name and the address the
   TUI will dial (a hostname, an IP, or a Tailscale name). The SSH fields are
   optional; fill them in only if you want Medulla to run the pairing command
   for you.
2. The tab shows a one-line command and copies it to your clipboard:

   ```
   medulla daemon HK1-…
   ```

3. Run that once on the machine. It says `paired with client … on udp/<port>`,
   then immediately re-execs itself as `medulla daemon --host`, which is what
   actually stays running — so the key is on that process's command line only
   for the instant it takes to record the pairing, not for the life of the
   daemon. Treat it like a credential anyway: it can still be read by another
   local user watching `ps`/`/proc` in that instant, or by anyone who can see
   the shell history of whoever typed the command in. Only pair from a
   machine and an account you trust. Running it via `s` (below), over a
   non-interactive `ssh`, keeps it out of the target's shell history
   entirely; typing it directly on a shared machine's own interactive shell
   does not. For a pair key that is never on a command line at all, use the
   [remoteHosts](remote-hosts.md) SSH-bootstrapped path instead, which mints
   its key over the SSH channel.
4. Back in the TUI, press `⏎` on the host. It goes **live** and lists what it
   offers: the coding CLIs installed there, plus a shell.

The key carries everything both ends need to agree on — your device's id, an id
for the host, a shared secret, and the UDP port — so the host needs nothing
else and never talks to anyone but you. If a key is lost or leaked, `i` on the
Hosts tab issues a fresh one; the old daemon stops being reachable the moment
you run the new command.

If the host's entry has SSH details, `s` runs the command over `ssh` for you
(key or agent authentication only). Either way, pairing is a one-time step:
`medulla daemon --host` brings a paired machine back up without the key, which
is what to put in a service unit or a tmux window.

## Reachability

The machine has to accept UDP on the port the key names from wherever the TUI
runs — the same requirement mosh has. A LAN, a VPN such as Tailscale, or a
public address with the port open all work. Ports are picked from 62001 upward,
above mosh's own range. If the daemon reports the port is taken, edit the host
(`e`), set another port, and re-issue the key (`i`).

## Switching hosts

`H` opens the host switcher from any tab. Pick **This device**, **All hosts**,
or one machine. Only the **Sessions** and **Workflows** tabs follow the
selection; Hosts, Feedback, Settings and login are always about this device.

- **All hosts** (the default) shows every machine's sessions on one rail,
  grouped under a header per host.
- **One host** narrows the rail to that machine, and `Ctrl`+`T` starts sessions
  there without asking which host. The Workflows tab shows *that machine's*
  workflow catalogue, runs and history, and `x` runs a workflow on it; editing
  and the copilot stay on this device, so authoring a remote workflow means
  switching back with `H`.

The header names the current host whenever the view is narrowed.

## What a session on a host gets

Whatever that machine is configured for: its own `[router]`, `[[hooks]]`,
`[attribution]` and `[[customHarnesses]]`, and the coding CLIs actually
installed there. A shell on a host is a real shell there, in the workspace the
host's entry names.
