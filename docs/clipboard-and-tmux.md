# Copying out of medulla, through tmux

Medulla is usually not running where you are sitting. A typical session is a
tmux on your laptop, an SSH connection, a second tmux on the box, and the TUI
inside that — with a coding-agent harness on a pty inside the TUI. Every copy has
to cross those layers to reach the clipboard you actually paste from.

## What can copy

- **A drag over the TUI** — the swept block is read back out of the rendered
  frame, so this works over any pane, including a harness pane.
- **`/copy` and the chat copy bindings** — the transcript or the last reply.
- **A short line** — a worker address, a command.
- **The harness itself** — anything the wrapped agent copies (OSC 52), including
  a `tmux load-buffer -w` run inside its own pane. Medulla is that harness's
  terminal, so its copy arrives here and is forwarded on rather than dropped.

## The routes a copy takes

Nothing in this chain can be confirmed from our side, so medulla does not pick
one route — it takes every route that could work, all carrying the same text:

1. **The OSC 52 escape**, written to our own stdout. Handled by the terminal at
   the far end of SSH, if every multiplexer in between forwards it.
2. **The same escape wrapped in tmux's DCS passthrough**, which a tmux with
   `allow-passthrough on` hands upwards verbatim — one hop out of the inner
   tmux, where the outer tmux then sees an ordinary OSC 52 from a pane.
3. **`tmux load-buffer -w`** against the socket in `$TMUX`, which needs no
   terminal capability and no configuration at all. This one always works, and
   worst case you paste from the tmux buffer with `prefix ]`.
4. **A local clipboard binary** (`pbcopy`, `wl-copy`, `xclip`, `xsel`), so a
   medulla running on the machine you are sitting at lands in the real
   clipboard.

The status line names what actually took it, e.g.
`Copied selection → clipboard (tmux buffer + OSC 52)`. "Sent … → terminal" means
only route 1 and 2 went out and nothing acknowledged them.

## Making the nested case reach your laptop

Routes 3 and 4 need no setup. For the escape to travel the whole way to the
terminal on your laptop, each tmux in the chain has to agree to pass it on:

```tmux
# On the box (the inner tmux): forward what medulla wraps for us, and take
# clipboard writes into our own buffer as well.
set -g allow-passthrough on
set -g set-clipboard on

# On the laptop (the outer tmux): accept OSC 52 from a pane and hand it to the
# terminal emulator.
set -g set-clipboard on
```

Your terminal emulator must also allow OSC 52 writes — iTerm2 and WezTerm do by
default, and some terminals gate it behind a setting.

`allow-passthrough` defaults to off in tmux 3.3 and later, which is why the
inner tmux is the layer that usually needs the change.

## Where this lives in the code

- `src/sdk/src/clipboard/` — the routes themselves; `tmux.rs` holds the
  passthrough wrapper, the `load-buffer` hop, and the OSC 52 parameter parsing.
- `src/tui/src/worker/pty/manager/clipboard.rs` — forwarding a harness's own
  copy out of its pane.
- `src/tui/src/ui/app/render/selection.rs` — the drag-to-select path.
