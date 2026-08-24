# Agent-harness wire contract

An **agent harness** is a long-lived agent that accepts natural-language
instructions, keeps a durable task board, delegates to connected agents, and
surfaces a status snapshot plus an event stream. When a backend fronts that
harness, its JSON crosses the wire to this client. The SDK represents those
public wire shapes as serde types so the SDK and TUI can decode them consistently.

The mirrors live in [`medulla::harness_contract`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_contract/).
Field names match the public JSON contract — every struct is
`#[serde(rename_all = "camelCase")]` and the status/state enums are lowercase —
and round-trip tests in `harness_contract/tests.rs` assert those names against
hand-written JSON literals. The format is versioned as a wire contract so clients
can validate compatibility independently of any implementation.

## Mirrored types

| Rust type | Notes |
| --- | --- |
| `TrackedTask`, `TrackedTaskStatus` | Status is `open`/`active`/`blocked`/`done`/`cancelled`. `createdAt`/`updatedAt` are ISO-8601 strings; `delegatedTaskIds` and `notes` are always-present arrays. |
| `HarnessStatus`, `HarnessState`, `HarnessUsage` | State is `idle`/`running`/`stopped`. `lastResult` is an opaque cycle-result payload (`serde_json::Value`). |
| `HarnessEvent` | Tagged by `kind`. The three lifecycle kinds (`instruction_queued`, `cycle_start`, `cycle_end`) are distinct variants; `task_board_changed` carries a `TrackedTask`; `cycle_event` wraps an opaque event payload. |
| `InstructionReceipt` | The serialisable `{ instructionId, cycleId }` fields. |
| `AgentBudgetMetadata` | The `metadata.budget` stamp on an agent descriptor. |
| `SeatHeadroom`, `WindowHeadroom` | Live seat headroom with a per-window map. |

### Two opaque payloads

`HarnessStatus::last_result` (`CycleResult`) and `HarnessEvent::CycleEvent { event }`
(`CycleEvent`) are kept as `serde_json::Value` rather than mirrored field by
field. Neither is a contract this client consumes; keeping them opaque preserves
them losslessly without coupling the client to their internals.

### Timestamp contrast: `AgentBudgetMetadata` vs `SeatHeadroom`

Both describe seat headroom, but the public contract formats their timestamps
differently:

- **`SeatHeadroom`** carries **epoch-milliseconds numbers** throughout
  (`primaryResetsAt`, `throttledUntil`, and each window's `resetsAt`).
- **`AgentBudgetMetadata`** — the roster-facing stamp the backend writes onto a
  descriptor — carries **`primaryResetsAt` as an ISO-8601 string**, formatted at
  the roster boundary.

## Reserved tool names

The harness composes its own memory and task-tracker modules eagerly, so a
business tool that reuses one of their names throws at construction. A
third-party module author must avoid these six names, exported as
`harness_contract::RESERVED_TOOL_NAMES` (with an `is_reserved_tool_name` helper):

```
task_create   task_update   task_list
memory_write  memory_read   memory_list
```

These names are part of the public harness contract.

## TUI rendering (read-only)

The Agents tab renders two harness surfaces, both additive and both degrading to
nothing when their payload is absent:

- **Task board.** When the backend runtime surfaces a `HarnessStatus`
  (`RuntimeSnapshot::harness`), the Agents transcript header shows a compact board
  — a per-status count summary (`tasks · open 2 · active 1 · done 3`) followed by
  one `glyph title` row per task. The pure helpers live in
  [`medulla::ui::harness`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/harness.rs).
- **Seat budget.** When a selected lane's agent descriptor carries a
  `metadata.budget` stamp (`AgentBudgetMetadata`), the header shows a one-line note
  — `seat Claude Max 5× · 1.2M left`, or `… · exhausted` when the seat is spent.

**Budget display is strictly read-only.** Seat CRUD (connecting, enabling, or
removing a user's BYO subscription seat) stays a backend REST concern and is not
built into the TUI. The TUI only decodes and shows the `metadata.budget` stamp
the backend already attaches to a descriptor.
