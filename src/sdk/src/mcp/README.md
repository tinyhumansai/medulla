# MCP

A Model Context Protocol server exposing workflow and session-scoped `fleet_*`
operations to spawned harnesses.

## Contents

- [`mod.rs`](./mod.rs) — JSON-RPC handling and the concurrent stdio server.
- [`attach.rs`](./attach.rs) — which sessions are served this server, the grant
  each one is minted, and how the registration reaches a harness over ACP or on
  a CLI's argv.
- [`types.rs`](./types.rs) — Shared MCP session state and policy types.
- [`backend/`](./backend/) — Offline and control-socket fleet backends.
- [`tools/`](./tools/) — Tool definitions and dispatch, including the
  session-scoped `fleet_*` family.
- [`tests/`](./tests/) — Protocol, policy, and workflow-tool unit tests.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data
structures in `types.rs`, focused unit tests in the owning directory module's
`tests.rs`, and preserve module-level Rust documentation as the API source of
truth. Cross-module MCP fleet behavior belongs in `src/sdk/tests/`.
