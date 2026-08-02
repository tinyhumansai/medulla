# Onboarding

First-run worker registration orchestration.

## Contents

- [`mod.rs`](./mod.rs) — First-run worker registration orchestration.
- [`tests.rs`](./tests.rs) — Unit tests for the onboarding orchestration: the env-owner chain, identity presence detection, and the headless auto-register path.
- [`types.rs`](./types.rs) — The onboarding module's data types: the `Registration` result, the `OnboardingContext` handed to an interactive UI, and the `OnboardingUi` callback the app injects to render the interactive screen.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
