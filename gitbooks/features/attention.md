---
description: >-
  Medulla reads every session's screen for the moments that need a person, and
  marks the row instead of making you look.
---

# Attention cues

An agent stopped on its own permission prompt looks exactly like one that is thinking hard: still running, still holding its terminal, saying nothing. In tmux you find out by looking. Medulla watches each session's screen for that state and marks it. The row turns the attention colour and pulses with a `⚠`, says what it is waiting for and for how long (`codex is asking permission · 42s`), the rail title says `⚠ 3 waiting on you`, and the tab bar carries a `⚠3` badge so a stuck session is visible from any tab.

## What it watches for

Most cues are read from what the harness paints, in order of specificity:

1. **Startup dialogs.** Trust and permission prompts that gate the whole session before it starts.
2. **Named prompts.** Distinctive phrases each CLI writes when it is asking. Claude Code's "No, and tell Claude what to do differently", Codex's "Allow Codex to…". Claude's plan-mode exit menu is recognised on its own, so the row says it finished planning and wants a decision rather than that it is asking something.
3. **Numbered menus.** A caret resting on a numbered option, or a bare `(y/n)`.
4. **Blocking errors.** A usage limit, an expired sign-in, a rejected credential. These print instead of a completed turn, so the work did not happen; the row says which.
5. **The terminal bell.** The fallback, for a prompt that is worded differently or not recognised.

One cue does not come from the screen. Claude Code raises a `Notification` lifecycle hook when it stops on a tool approval, an MCP elicitation form, or a background agent waiting on you, and Medulla treats that in the session's hook log as a wait. The screen reader still wins when it can name something more specific; the hook catches a stop the screen paints nothing recognisable for. Codex does not report it, so this cue is Claude's alone.

Two states are taken from the process rather than the screen. A harness that **died** leaves its terminal frozen on whatever it last painted, often an ordinary composer, so Medulla turns the row red and says what happened (`codex exited with 137`). A dispatched task that **finished** leaves the session standing for you to read, which looks identical to a session nobody has used; that row shows `✓ … finished — read and release`. It is shown but not counted in the `⚠` badge, since nothing is held up while it waits, and a badge that ticks up on every success is a badge you learn to ignore.

## The spinner

A harness thinking hard writes nothing, so for a long time a busy session and an idle one looked the same in the rail. Medulla now reads the harness's own progress line and spins the row glyph for exactly as long as the turn lasts. Idle is `●`, busy is `⠋`.

## What clears a mark

Attaching to the session. That means someone is now dealing with it. A named prompt comes straight back on the next sample if the harness is in fact still asking, so nothing is hidden. A bell does not, because a ring has no second frame to keep it alive.

## Colour and pulse

The pulse is counted off Medulla's own render clock rather than the terminal's blink attribute, which most terminals and every multiplexer ignore. A failure pulses red regardless of the attention colour, so "it is asking you something" and "it broke" are never the same colour. Both are yours to set:

```toml
[theme]
attention = "yellow"
attentionBlink = true
attentionBlinkSeconds = 1.0   # one bright-to-dim cycle; clamped to 0.2–10.0
```

Settings › Appearance edits all three live.

## Remote sessions

Cues on a remote host travel the reliable channel with the rest of the row state, so a remote session lights up in the rail the same way a local one does, whether or not its screen is the one streaming. The "for how long" reading is measured from when your machine first saw the cue, not from the remote clock, so a host with skewed time cannot show a fresh prompt as stuck for hours.

## Read next

* [Sessions](sessions.md) for the rail and the glyphs.
* [Configuration › Theme](../developers/configuration.md#theme) for the colour keys.
