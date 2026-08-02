# Config

`medulla.tui.json`-compatible config — the subset the TUI reads, plus a `backend` section for the HTTP runtime. Permissive: missing fields take defaults, unknown fields are ignored.

## Contents

- [`core_socket/`](./core_socket/) — Core (`medulla-serve`) socket resolution and validation: where the unix socket path the core runtime attaches to comes from (`--core-socket` flag, `MEDULLA_CORE_SOCKET` env var, `[core]` config section, or the default runtime dir), and the fail-fast check that a resolved path is actually attachable *before* `CoreRuntime::attach` is handed a value it can only spin on. AGENTS.md treats socket paths as untrusted configuration to be validated at boundaries — this module is that boundary.
- [`core_socket_tests.rs`](./core_socket_tests.rs) — Unit tests for core-socket resolution and validation: the path/request precedence on `LoadedConfig`, the source naming, and the fail-fast `validate_core_socket` boundary check.
- [`custom_harnesses.rs`](./custom_harnesses.rs) — Named OpenRouter-backed harness presets (`[[customHarnesses]]`): a coding CLI as the agent runtime with OpenRouter supplying the model and credential. Secrets are referenced by environment-variable name and never stored in the document. All three built-in harnesses are accepted — including OpenCode, whose native OpenRouter path is exactly the one that bypasses `crate::inference_proxy`.
- [`custom_harnesses_tests.rs`](./custom_harnesses_tests.rs) — Unit tests for preset parsing, normalization, per-harness endpoint and tier-environment mapping, key-presence checks, and layered loading.
- [`load_tests.rs`](./load_tests.rs) — Unit tests for layered config discovery, parsing, merging, and env overrides.
- [`load.rs`](./load.rs) — Layered config discovery, parsing, and merge — the `load_config` entry point.
- [`mod.rs`](./mod.rs) — `medulla.tui.json`-compatible config — the subset the TUI reads, plus a `backend` section for the HTTP runtime. Permissive: missing fields take defaults, unknown fields are ignored.
- [`persist_tests.rs`](./persist_tests.rs) — Unit tests for onboarding-state persistence (`super::persist`).
- [`persist.rs`](./persist.rs) — Writing individual config sections back to disk.
- [`types_tests.rs`](./types_tests.rs) — Unit tests for the config data model: serde defaults/parsing and derived labels on `LoadedConfig`. Core-socket resolution/validation tests live in `super::core_socket_tests`.
- [`types.rs`](./types.rs) — The config data model: every `[section]` the TUI reads, plus the `LoadedConfig` result that pairs the parsed config with its provenance.
- [`urls_tests.rs`](./urls_tests.rs) — Unit tests for endpoint base-URL resolution and display-host formatting.
- [`urls.rs`](./urls.rs) — Endpoint base-URL constants and their environment-aware resolvers.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
