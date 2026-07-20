//! Rust mirrors of the medulla-v1 agent-harness wire contract.
//!
//! The orchestration library (the TypeScript `medulla-v1` package) exposes an
//! *agent harness*: a long-lived agent that accepts natural-language
//! instructions, maintains a durable task board, delegates to connected agents,
//! and surfaces a `HarnessStatus` snapshot plus a `HarnessEvent` stream. When a
//! backend fronts that harness, its JSON crosses the wire to this client. This
//! module mirrors those shapes as serde types so the SDK and TUI can decode them
//! without re-deriving the contract by hand.
//!
//! The [`types`] submodule holds the shapes; this file owns the module docs, the
//! reserved tool-name vocabulary, and the public re-exports. Round-trip tests
//! against hand-written JSON literals live in the sibling [`tests`] module.
//!
//! Source of truth (medulla-v1): `src/ports/taskTracker.ts`, `src/agent/harness.ts`,
//! `src/core/taskTools.ts`, `src/core/memoryTools.ts`, `src/ports/budgets.ts`,
//! and `src/core/budgetRoster.ts`. Field names match that JSON exactly.

mod types;

#[cfg(test)]
mod tests;

pub use types::{
    AgentBudgetMetadata, HarnessEvent, HarnessState, HarnessStatus, HarnessUsage,
    InstructionReceipt, SeatHeadroom, TrackedTask, TrackedTaskStatus, WindowHeadroom,
};

/// The tool names the harness reserves for its built-in memory and task-tracker
/// modules. A third-party module author registering business tools must avoid
/// these names — the harness composes its own modules eagerly
/// and a collision throws at construction. Kept in sync with `core/taskTools.ts`
/// and `core/memoryTools.ts`.
pub const RESERVED_TOOL_NAMES: [&str; 6] = [
    "task_create",
    "task_update",
    "task_list",
    "memory_write",
    "memory_read",
    "memory_list",
];

/// Whether `name` collides with a harness-reserved tool name.
pub fn is_reserved_tool_name(name: &str) -> bool {
    RESERVED_TOOL_NAMES.contains(&name)
}
