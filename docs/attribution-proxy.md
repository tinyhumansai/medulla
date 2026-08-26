# OpenRouter attribution proxy

When Medulla runs a harness against OpenRouter, the traffic is routed through a
loopback proxy Medulla owns rather than going to `openrouter.ai` directly.

## Why

OpenRouter decides which application to credit by reading two request headers:

| Header | Purpose |
| --- | --- |
| `HTTP-Referer` | the application's URL |
| `X-Title` | the application's name, shown on OpenRouter's app leaderboard |

Claude Code, Codex and OpenCode each set their own. Left alone, a run Medulla
scheduled, prompted and paid for is credited to the coding CLI it happened to
use. The proxy reverses that: on every outbound request it strips the harness's
claim and substitutes Medulla's.

## What the harness sees

Nothing about the harness changes except where it points. Medulla already
injects `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` at the spawn seam; those now
name a loopback port:

```
ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/anthropic     # Claude Code
OPENAI_BASE_URL=http://127.0.0.1:<port>/openai           # Codex, OpenCode
ANTHROPIC_AUTH_TOKEN / OPENAI_API_KEY = mdl-<random>     # NOT the real key
```

The two mounts exist because the harnesses speak different dialects:
`/anthropic` forwards to `https://openrouter.ai/api`, `/openai` to
`https://openrouter.ai/api/v1`.

## The key never reaches the harness

The child is given a machine-local token, and the real OpenRouter key is removed
from its environment entirely: both the variable your preset names in
`apiKeyEnv` and the default `OPENROUTER_API_KEY`. For an endpoint-only preset,
the selected harness's inherited credential (`ANTHROPIC_AUTH_TOKEN` or
`OPENAI_API_KEY`) is used as the upstream key and scrubbed too.

Withholding the key is what makes the rewrite hold. A harness
holding the real key could ignore the base URL and call OpenRouter itself; a
harness holding only a loopback token has nothing to call it with. The token
authenticates against this process, on this machine, and is worthless anywhere
else.

## On the wire

| Header | From the harness | To OpenRouter |
| --- | --- | --- |
| `Authorization` | `Bearer mdl-<token>` | `Bearer <your OpenRouter key>` |
| `HTTP-Referer` | `https://claude.ai` | `http://medulla.tinyhumans.ai/` |
| `X-Title` | `Claude Code` | `Medulla` |
| `x-openrouter-*`, `x-app-*` | whatever it set | dropped |
| `anthropic-version`, `anthropic-beta`, `content-type`, `accept`, `user-agent` | as sent | forwarded unchanged |

`User-Agent` is deliberately left alone: attribution is the referer/title pair,
and rewriting the UA would misreport the client to the upstream.

Responses stream straight back. A harness turn is an SSE token stream, and the
proxy relays chunks as they arrive rather than collecting the response first.

## Lifecycle

The listener binds `127.0.0.1:0` on first use and lives for the process, on its
own thread and runtime so harness traffic cannot compete with the terminal UI.
One listener serves every harness, and credentials are separated by token rather
than by port, so two presets sharing an `apiKeyEnv` share a token and two with
different keys do not.

Nothing is exposed off the machine: non-loopback peers are dropped at accept,
and an unrecognized token is refused with a 401 before any upstream request is
made.

## Coverage

Routing applies to every Medulla-launched run that resolves to an OpenRouter
endpoint:

- headless daemon tasks, including the ACP transport;
- watched PTY sessions on the local host;
- harnesses an operator opens by hand in the TUI.

All three spawned harnesses are covered. OpenCode is accepted as a
`customHarnesses` base even though it can reach OpenRouter natively, because that
native path is exactly the one this proxy needs to take over.

The local in-process harness is covered too, by the same mechanism through a
different call shape. Naming `openhuman` as a node's harness no longer dispatches
into a separate core; it runs the turn in-process on the vendored `tinyagents`
loop, with Medulla's own tools. That turn is not a child process, so there is no
environment to inject into and nothing to scrub: Medulla resolves the preset's
key, exchanges it for a loopback token, and hands the turn the mount and the
token directly as the route it calls inference on. Unlike a spawned harness, this
one has no ambient inference configuration to fall back to — with no router
preset naming an endpoint and a model, the turn refuses to run rather than
resolving one some other way. As with a spawned harness, the turn is given the
token and never the OpenRouter key.

One limitation applies. Medulla injects environment variables at the spawn seam
and never writes a harness's own configuration file. A harness you have
separately configured to reach OpenRouter through its own config still bypasses
this proxy, and that traffic will be attributed to the harness.

## Configuration and troubleshooting

There is nothing to turn on. Any `[router]` block or `[[customHarnesses]]`
preset whose endpoint resolves to `openrouter.ai` is routed; anything else (a
private gateway, a self-hosted endpoint) is left exactly as configured.

Routing is skipped, with no error, when the key variable is unset or blank: a
proxy with no upstream credential could only turn a working run into a 401.

`MEDULLA_OPENROUTER_URL` overrides the upstream root. It exists so the test
suite can run offline against a mock; operators should not normally set it.

If the proxy cannot bind a loopback socket, the spawn fails with
`could not start the local attribution proxy: …` rather than falling back to a
direct connection. A silent fallback would send unattributed traffic and put
the key back in the child, undoing both things this exists to do.
