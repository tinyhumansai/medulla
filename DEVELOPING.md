# Developing Medulla

The Rust workspace, documentation, installers, and GitHub Actions live in this
repository. The hosted orchestration service is operated separately; the
`--mock` runtime supports offline development and tests.

## Quick start

```sh
make init                       # initialize vendored crates and Rust tooling
cargo run                       # start the TUI
cargo run -- --mock             # run the offline demo
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --all-targets
```

See [Contributing](gitbooks/developers/contributing.md) for the detailed test,
coverage, and contribution workflow. The engineering specifications referenced
by source comments are in [`docs/`](docs/).

## Releases

The **Release** workflow is dispatched from `main`. It reads the workspace
version, creates a matching `vX.Y.Z` tag, builds packages for Linux x86_64,
Linux aarch64, macOS Apple Silicon, and Windows x86_64, then publishes those
artifacts and `latest.json` as a GitHub Release in this repository. The workflow
uses the repository's standard `GITHUB_TOKEN`; it requires no organization app
secrets. Bump the workspace version and lockfile in a reviewed pull request
before dispatching a release.

### Crash-reporting credentials

Crash reporting (Sentry) reads its DSN at **compile time** through
`option_env!`, so a plain local `cargo build` produces a binary with crash
reporting inert. `Release` passes it into the build step and emits a
`::warning::` when it is empty:

| Build-time variable | Source in `Release` | Effect when unset |
| --- | --- | --- |
| `MEDULLA_SENTRY_DSN` | `vars.MEDULLA_SENTRY_DSN` | no crash reporting |

At run time `MEDULLA_SENTRY_DSN` overrides the baked-in DSN,
`MEDULLA_SENTRY_ENVIRONMENT` overrides the Sentry environment (default
`production` for release builds, `development` for debug), and
`MEDULLA_ANALYTICS_DISABLED=1` turns crash reporting off.

To verify a build's crash-report wiring end to end, run the hidden
`medulla sentry-test` command: it sends one informational event, prints its
event id and the ingestion endpoint's HTTP status, and exits non-zero if
crash reporting is inactive or the event was not accepted.

Debug symbols are not uploaded to Sentry yet; release binaries symbolicate
in-process from whatever debug info they carry.
