# Defaults

The built-in agent-template catalog: the coding roles every install knows without any files on disk.

## Contents

- [`code-reviewer.toml`](./code-reviewer.toml) — Defines the built-in Code Reviewer agent template.
- [`debugger.toml`](./debugger.toml) — Defines the built-in Debugger agent template.
- [`doc-writer.toml`](./doc-writer.toml) — Defines the built-in Doc Writer agent template.
- [`implementer.toml`](./implementer.toml) — Defines the built-in Implementer agent template.
- [`merge-resolver.toml`](./merge-resolver.toml) — Defines the built-in Merge Resolver agent template.
- [`mod.rs`](./mod.rs) — The built-in agent-template catalog: the coding roles every install knows without any files on disk.
- [`plan-writer.toml`](./plan-writer.toml) — Defines the built-in Plan Writer agent template.
- [`pr-manager.toml`](./pr-manager.toml) — Defines the built-in Pr Manager agent template.
- [`refactorer.toml`](./refactorer.toml) — Defines the built-in Refactorer agent template.
- [`repo-orchestrator.toml`](./repo-orchestrator.toml) — Defines the built-in Repo Orchestrator agent template.
- [`test-writer.toml`](./test-writer.toml) — Defines the built-in Test Writer agent template.
- [`triager.toml`](./triager.toml) — Defines the built-in Triager agent template.
- [`verifier.toml`](./verifier.toml) — Defines the built-in Verifier agent template.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
