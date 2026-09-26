# Repository Guidelines

## What this repository contains

This is the public Medulla repository. It contains the Rust client workspace,
its vendored dependency submodules, tests and e2e harnesses, documentation,
GitBook sources, installers, and release workflows. The hosted orchestration
service is operated separately; do not add service-side implementation here.

- `src/` — the `medulla-link`, `medulla` SDK, and `medulla-tui` crates.
- `vendor/` — pinned source submodules used by the Rust workspace.
- `docs/` and `gitbooks/` — engineering specs and published documentation.
- `.github/`, `scripts/`, `e2e/`, and `examples/` — CI/release automation,
  development helpers, integration suites, and examples.

## Working rules

- Make changes on a feature branch in a worktree. Run `worktree <slug>` before
  editing and work inside the reported `./worktrees/<slug>` path.
- Keep source, tests, docs, and workflows together when a change affects them.
- Preserve vendored dependency pins unless deliberately updating a dependency.
- Keep CI and release workflows usable with standard GitHub Actions permissions;
  never require organization-only secrets for routine builds or releases.
- Never commit secrets, `.env` files, or machine-local configuration.

## Checks

Initialize dependencies and developer tooling with `make init`. The primary
checks are:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --all-targets
```

Use `make ci` to run the full local gate. See [DEVELOPING.md](DEVELOPING.md)
and [Contributing](gitbooks/developers/contributing.md) for the development
workflow.
