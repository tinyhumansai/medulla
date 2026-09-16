# One terminal, not a pile of them

Coding agents are good at one task at a time. The moment you want three of them going, on two machines, the tooling around them starts to show its age. This page is about what that tooling cannot do, and what Medulla does instead.

## The setup everyone ends up with

You open a terminal for Claude Code in the frontend repo. Another for Codex in the API. A third is an ssh session to the build box, where a fourth agent is running a migration. Somewhere in there is a shell you use to check on things. If you are organised, all of this is in tmux, and you have a layout you can mostly remember. If the build box is far away you are on mosh so the session survives your laptop going to sleep.

This works. It works for three or four agents, and it works as long as you are willing to be the scheduler.

## What a multiplexer cannot tell you

tmux gives you panes and has no idea what is in them. To tmux, an agent that is stopped on a permission prompt looks the same as one that is thinking hard, and both look the same as one that crashed twenty minutes ago and left its last screen on the terminal. You find out by cycling through the panes and reading each one. That is a job that grows with every agent you add, and it holds for about as long as your attention does.

ssh has a different problem. Each remote machine is a separate connection, with its own window, its own tmux inside it, and its own way of going stale when you close the lid. mosh fixes the stale part and nothing else.

## What Medulla does instead

Medulla runs the same agent processes on the same kind of pseudo-terminal, and then reads them. Every session has its own terminal emulator kept live in the background, and Medulla watches all of them for the things that mean an agent needs a person: a permission prompt, a startup dialog, a numbered menu, a `(y/n)`, a usage limit, a terminal bell, a process that exited, a turn that finished and is waiting to be read. Those become a `⚠` on the row and a count in the rail title. You look at the one that needs you and leave the rest alone.

Remote machines are rows in the same rail. The first time you open a session on a host, Medulla runs your own `ssh` to start a small daemon there and bring back a key. Everything after that is UDP between the two machines, mosh-style, so a closed lid or a new Wi-Fi network does not cost you the session. You never hold an ssh window open, and you never nest a tmux inside one.

Attaching is one key. `Ctrl-]` hands your keyboard to the selected session and `Ctrl-]` takes it back; while you are attached, every other key goes to the agent, so its own shortcuts keep working. Opening a new session is `Ctrl-T`, and the twentieth is no different from the second.

## What Medulla is not

Medulla does not decide what work to hand out or which agent should do it. There is no model sitting above your sessions, reading their output and planning. You open sessions, you type into them, and Medulla keeps them running and tells you when one needs you. The agents are the real CLIs with their own credentials, running in the directories you chose, and they behave exactly as they do outside Medulla.

## Read next

* [Sessions](features/sessions.md) for the rail, the keys, and what a row shows.
* [Remote machines](features/remote-hosts.md) for adding a host.
* [Attention cues](features/attention.md) for the full list of what Medulla watches for.
