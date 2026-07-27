# ACP harness transport

Medulla can communicate with coding harnesses through version 1 of the
[Agent Client Protocol](https://agentclientprotocol.com/). This gives the daemon
one lifecycle and event stream for Claude Code, Codex, OpenCode, and future ACP
agents instead of teaching the orchestration layer each harness's private JSONL
format.

Set the protocol in the daemon environment:

```sh
export MEDULLA_HARNESS_PROTOCOL=acp
medulla daemon --headless
```

The daemon launches these ACP servers over stdio:

| Harness | ACP server |
| --- | --- |
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp@latest` |
| Codex | `npx -y @agentclientprotocol/codex-acp@latest` |
| OpenCode | `opencode acp` |

The ACP client performs `initialize`, creates or loads a session, sends
`session/prompt`, streams `session/update` notifications into Medulla status
events, answers permission requests, and sends `session/cancel` when a remote
task is aborted. Permission requests are denied unless the daemon was started
with its existing skip-permissions option.

The legacy provider JSONL transport remains the default during migration.
Removing `MEDULLA_HARNESS_PROTOCOL` returns to it immediately, which permits a
host-by-host rollout without changing the Medulla task-frame protocol.

ACP session IDs are returned through the same `RunTaskResult::session_id` field
used by the legacy adapters, so conversation resumption does not depend on a
provider-specific transcript shape.
