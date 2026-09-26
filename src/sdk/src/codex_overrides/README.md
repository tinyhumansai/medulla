# Codex Overrides

Codex `-c` config overrides that make a routed Codex run actually reach a non-OpenAI model.

An `OPENAI_BASE_URL` on its own is not enough for Codex 0.146: a signed-in ChatGPT account wins over the routed key, and a model Codex has no catalog entry for is described with fallback metadata whose `custom` tool shapes a `function`-only provider rejects with a 400. A preset that sets `codexOverrides` gets a namespaced provider block, `preferred_auth_method = "apikey"`, and a catalog entry derived at spawn time from the catalog the installed Codex cached for itself. Nothing is written to the operator's `~/.codex/config.toml`, and `codex login` is untouched.

## Contents

- [`mod.rs`](./mod.rs) — The spawn-seam entry point: the `-c key=value` argv a routed Codex run needs, the opt-in and endpoint gates it returns empty on, and the environment variables a preset publishes its knobs through.
- [`catalog.rs`](./catalog.rs) — Deriving the model-catalog entry from `$CODEX_HOME/models_cache.json`, including the three tool-shape fields that have to be turned off, and the atomic write into Medulla's state directory.
- [`tests.rs`](./tests.rs) — Unit tests for the emitted overrides, the derived catalog, the gates, TOML quoting, and the error a missing Codex cache raises.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
