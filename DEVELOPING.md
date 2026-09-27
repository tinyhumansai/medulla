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

### Telemetry credentials

Crash reporting (Sentry, `medulla::observability`) and product analytics
(OpenPanel, `medulla::analytics`) read their credentials at **compile time**
through `option_env!`, so a plain local `cargo build` produces a binary with
both inert. `Release`'s build job runs in the `Production` environment (which
only `main` may use), passes these into the build step, and emits a
`::warning::` when the DSN or the OpenPanel secret is empty:

| Build-time variable | Source in `Release` | Effect when unset |
| --- | --- | --- |
| `MEDULLA_SENTRY_DSN` | `vars.MEDULLA_SENTRY_DSN` | no crash reporting |
| `MEDULLA_OPENPANEL_CLIENT_SECRET` | `secrets.MEDULLA_OPENPANEL_CLIENT_SECRET` | no analytics |
| `MEDULLA_OPENPANEL_CLIENT_ID` | `vars.MEDULLA_OPENPANEL_CLIENT_ID` | Medulla project's id |
| `MEDULLA_OPENPANEL_API_URL` | `vars.MEDULLA_OPENPANEL_API_URL` | `https://panel.tinyhumans.ai/api` |

The OpenPanel secret is required because the self-hosted instance rejects a
native client that sends only a client id (`401 Invalid cors or secret`). The
OpenPanel values are never read at run time.

At run time `MEDULLA_SENTRY_DSN` overrides the baked-in DSN,
`MEDULLA_SENTRY_ENVIRONMENT` overrides the Sentry environment (default
`production` for release builds, `development` for debug), and
`MEDULLA_ANALYTICS_DISABLED=1` turns off both crash reporting and analytics.

Analytics records `application_started` (OS and architecture), `signed_in`,
`signed_out`, `screen_viewed` (top-level tab name), `ui_action` (only that a
command was dispatched), and `token_usage_reported` (counts only), each tied to
the opaque account id — never prompts, paths, arguments, or configuration. The
payloads match the `openpanel_rust` SDK's wire format; Medulla posts them over
its own reqwest/rustls stack because the SDK can only be configured from a
`.env` file and the process environment.

To verify a build's wiring end to end, run the hidden diagnostics:

- `medulla sentry-test` sends one informational event, prints its event id and
  the ingestion endpoint's HTTP status, and exits non-zero if crash reporting
  is inactive or the event was not accepted.
- `medulla analytics-test` sends one `analytics_test` event and prints
  OpenPanel's HTTP status (`401` means the baked-in secret is wrong); it exits
  non-zero if analytics is inactive or the event was not accepted.

To try analytics locally, export the secret for the build only, e.g.
`MEDULLA_OPENPANEL_CLIENT_SECRET=... cargo build`, and never commit it.

Debug symbols are not uploaded to Sentry yet; release binaries symbolicate
in-process from whatever debug info they carry.
