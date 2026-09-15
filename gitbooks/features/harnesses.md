---
description: >-
  A harness is a coding agent Medulla can open in a session. The real CLI runs,
  with its own credentials, in the directory you chose.
---

# Harnesses

A harness is the thing a session runs. Medulla opens the real coding CLI on a real pseudo-terminal, in the directory you picked, with that machine's credentials. It behaves exactly as it would if you had typed its name into a terminal, because that is what happened.

## What the picker offers

| Harness | How it is found | What is launched |
| --- | --- | --- |
| Claude Code | `claude` on `PATH` | `claude`, full screen |
| Codex | `codex` on `PATH` | `codex`, full screen |
| OpenCode | `opencode` on `PATH` | `opencode tui` |
| OpenHuman | `openhuman` on `PATH` | `openhuman tui` |
| Shell | every shell installed on the machine, your `$SHELL` first | the shell, as a plain terminal |

The CLIs are detected by searching `PATH`. `MEDULLA_CLAUDE_BIN`, `MEDULLA_CODEX_BIN`, `MEDULLA_OPENCODE_BIN`, and `MEDULLA_OPENHUMAN_BIN` point at a specific binary instead. `MEDULLA_SHELL_BIN` pins which shell leads the list.

A shell session is yours alone. Nothing dispatches into it and no automation types into it; it exists so you can have a terminal beside your agents without leaving the window.

On a remote host the list is whatever that machine has installed, plus the presets in its own config.

## Permission prompts

A session you open yourself launches with the CLI's permission prompts on, so Claude Code asks before editing and Codex asks before running commands, and Medulla marks the row when they do. That is the point of an attended session: there is someone to answer.

Set `[harness] skipPermissions = true` to launch with the bypass flag instead (`--dangerously-skip-permissions` for Claude Code, `--dangerously-bypass-approvals-and-sandbox` for Codex). OpenCode, OpenHuman, and shells have no such flag.

## Running another model through a CLI

A custom harness preset runs an OpenRouter model through one of the CLIs, so the CLI stays the agent and OpenRouter supplies the model. DeepSeek through Claude Code, for example:

```toml
[[customHarnesses]]
id = "deepseek"
name = "DeepSeek via Claude"
baseHarness = "claude"              # claude | codex | opencode | openhuman
model = "deepseek/deepseek-chat"
fastModel = "deepseek/deepseek-chat"
apiKeyEnv = "OPENROUTER_API_KEY"
```

Presets appear in the picker by name, after the CLIs. `apiKeyEnv` names an environment variable; the key itself never goes in the file. Claude presets map `model` onto Opus and `fastModel` onto the Sonnet and Haiku tiers so sub-agents also use the preset; Codex and OpenCode get the model through their normal `-m` argument.

The CLI never sees the real key. Medulla starts a loopback proxy, hands the CLI a machine-local token, and forwards to OpenRouter with Medulla's own attribution headers. `providerOnly` pins which of OpenRouter's serving providers may answer, which matters because one model is offered at prices that differ by more than an order of magnitude. [Configuration › Custom harness presets](../developers/configuration.md#custom-harness-presets) has every field.

A preset with `baseHarness = "openhuman"` does not spawn a CLI at all. It runs the turn in-process on Medulla's own agent loop, so there is nothing to install, and it is only offered when the key its `apiKeyEnv` names is actually set.

A `[router]` section points every harness at one OpenAI-compatible gateway instead, for centralised metering. See [Attribution and routing](../developers/attribution-and-routing.md).

## What a session gets from Medulla

Each launched harness is handed an MCP server, `medulla mcp`, over a per-session socket and grant token, so it can run saved [workflows](workflows.md) and report lifecycle events back. Claude Code also gets a `Notification` hook, which is how Medulla notices a stop the screen does not paint anything recognisable for. `[hookDefaults] enabled = false` turns Medulla's own hooks off; `[[hooks]]` adds your own. See [Configuration](../developers/configuration.md#hooks).

Git commits made from a session carry a `Co-authored-by: Medulla` trailer unless `[attribution] commit = false`.

## Read next

* [Sessions](sessions.md) for opening, attaching, and closing.
* [Configuration](../developers/configuration.md) for `[harness]`, `[[customHarnesses]]`, and `[router]`.
* [Environment variables](../developers/environment-variables.md) for the per-harness overrides.
