# Inference proxy

A loopback HTTP proxy that owns Medulla's OpenRouter attribution.

## Why

OpenRouter credits an application by reading the `HTTP-Referer` and `X-Title`
request headers. Claude Code, Codex and OpenCode each send their own, so a run
Medulla orchestrated and paid for is credited to the harness. Pointing the child
at a proxy Medulla controls is what reverses that: the harness's attribution
headers are stripped and Medulla's are substituted on the way out.

The child is also handed a **loopback token** rather than the OpenRouter key, and
the spawn seam scrubs the real key from its environment. Without that a harness
could simply call OpenRouter directly and the rewrite would be decorative.

## Shape

```
child harness                 this module                    OpenRouter
  ANTHROPIC_BASE_URL ─┐
  = 127.0.0.1:P/anthropic ──► /anthropic/… ──► https://openrouter.ai/api/…
  OPENAI_BASE_URL   ─┘
  = 127.0.0.1:P/openai   ──► /openai/…    ──► https://openrouter.ai/api/v1/…

  Authorization: Bearer mdl-<token>   ──►   Authorization: Bearer sk-or-<real>
  HTTP-Referer: https://claude.ai     ──►   HTTP-Referer: http://medulla.tinyhumans.ai/
  X-Title: Claude Code                ──►   X-Title: Medulla
```

One listener serves the whole process. The token, not the port, separates
credentials: two presets sharing an `apiKeyEnv` share a token, two with
different keys do not.

## Contents

- [`mod.rs`](./mod.rs) — module docs and public wiring.
- [`types.rs`](./types.rs) — endpoint, routing, handle, registry, and dialect
  data types.
- [`lifecycle.rs`](./lifecycle.rs) — proxy startup, token minting, and the shared
  process-wide listener.
- [`routing.rs`](./routing.rs) — provider-scoped router and child-environment
  rewriting at the spawn seam.
- [`headers.rs`](./headers.rs) — the pure request-header rewrite. All attribution
  policy lives here: what is stripped, what is injected, what is forwarded
  verbatim.
- [`serve.rs`](./serve.rs) — the accept loop, the loopback-peer and token guards,
  mount-to-upstream mapping, and the bidirectional streaming forward.
- [`tests.rs`](./tests.rs) — offline unit tests for the rewrite, host
  recognition, routing and token minting. Socket-level behaviour is covered by
  `src/sdk/tests/e2e_attribution_proxy.rs`.

## Integration

Spawn seams call `route_spawn(provider, router, env)`, which atomically replaces
an OpenRouter-bound provider's endpoint, injects its loopback token, and scrubs
the upstream credential. Nothing downstream changes: `RouterConfig` and
`crate::tinyplace::env::router_env` perform the same child injection, simply
pointed at loopback.

`MEDULLA_OPENROUTER_URL` overrides the upstream so tests stay offline.

## Limitation

This repo's boundary is env injection at the spawn seam; Medulla never writes a
harness's on-disk configuration. Attribution is therefore guaranteed for
Medulla-routed runs only — a harness the operator separately configured to reach
OpenRouter through its own config file still bypasses this proxy.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data
structures in `types.rs`, focused unit tests in `tests.rs`, and preserve the
module-level Rust documentation as the API source of truth.
